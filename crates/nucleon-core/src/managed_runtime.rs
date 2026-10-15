//! Install complete, self-contained native runtimes as retained immutable generations.
use crate::{
    fs_util::atomic_write_file,
    paths,
    runtime::{ExecutionMode, ResolvedRuntime},
    staging,
};
use anyhow::{Context, Result, ensure};
use object::Object;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

pub const RECEIPT: &str = ".nucleon-install.json";
const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Entry {
    Directory,
    File {
        sha256: String,
        size: u64,
        mode: u32,
    },
    Link {
        target: PathBuf,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    generation: String,
    source: PathBuf,
    #[serde(default)]
    archive: Option<ArchiveOrigin>,
    files: BTreeMap<PathBuf, Entry>,
    #[serde(default)]
    identities: BTreeMap<PathBuf, staging::Identity>,
    native_dependencies: BTreeMap<PathBuf, Vec<String>>,
    license_files: Vec<PathBuf>,
    distribution: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct ArchiveOrigin {
    location: String,
    sha256: String,
}
fn inventory(root: &Path) -> Result<BTreeMap<PathBuf, Entry>> {
    let mut files = BTreeMap::new();
    let mut size = 0u64;
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .sort_by_file_name()
    {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?.to_path_buf();
        if relative == Path::new(RECEIPT) {
            continue;
        }
        ensure!(
            files.len() < 50000,
            "Runtime contains too many filesystem entries"
        );
        let meta = fs::symlink_metadata(path)?;
        let value = if meta.is_symlink() {
            let target = fs::read_link(path)?;
            ensure!(
                !target.is_absolute() && path.canonicalize()?.starts_with(root),
                "Runtime symlink escapes its root: {}",
                relative.display()
            );
            Entry::Link { target }
        } else if meta.is_dir() {
            Entry::Directory
        } else {
            ensure!(
                meta.is_file(),
                "Unsupported runtime filesystem entry: {}",
                path.display()
            );
            ensure!(
                meta.mode() & 0o6000 == 0,
                "Runtime contains a privileged executable: {}",
                path.display()
            );
            size = size
                .checked_add(meta.len())
                .context("Runtime size overflow")?;
            ensure!(size <= MAX_BYTES, "Runtime exceeds installed size limit");
            Entry::File {
                sha256: staging::digest(path)?,
                size: meta.len(),
                mode: meta.mode() & 0o777,
            }
        };
        files.insert(relative, value);
    }
    Ok(files)
}
fn validate_paths(files: &BTreeMap<PathBuf, Entry>) -> Result<()> {
    ensure!(
        files.contains_key(Path::new(crate::runtime::DESCRIPTOR)),
        "Managed runtime has no descriptor"
    );
    for relative in files.keys() {
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "Invalid managed runtime inventory path"
        );
    }
    Ok(())
}

/// Audit load commands without loading code. System dependencies are provided by
/// macOS; every other native dependency must resolve inside this generation.
fn audit(
    runtime: &ResolvedRuntime,
    files: &BTreeMap<PathBuf, Entry>,
) -> Result<BTreeMap<PathBuf, Vec<String>>> {
    use object::read::macho::LoadCommandVariant;
    let root = &runtime.root;
    let mut result = BTreeMap::new();
    for (relative, entry) in files {
        if !matches!(entry, Entry::File { .. }) {
            continue;
        }
        let path = root.join(relative);
        let mut magic = [0u8; 4];
        let mut f = fs::File::open(&path)?;
        if f.read(&mut magic)? != 4
            || !matches!(
                u32::from_be_bytes(magic),
                0xfeedfacf | 0xcffaedfe | 0xcafebabe | 0xbebafeca | 0xcafebabf | 0xbfbafeca
            )
        {
            continue;
        }
        let bytes = fs::read(&path)?;
        for slice in crate::runtime_checks::slices(&bytes)? {
            let image = object::File::parse(slice)?;
            if image.architecture() != runtime.arch() {
                continue;
            }
            let object::File::MachO64(macho) = image else {
                continue;
            };
            let mut rpaths = vec![
                runtime.unix_dir.clone(),
                root.join("lib"),
                root.join("lib/vulkan"),
                path.parent().unwrap().to_path_buf(),
            ];
            let expand = |text: &str| -> PathBuf {
                if let Some(t) = text.strip_prefix("@loader_path/") {
                    path.parent().unwrap().join(t)
                } else if let Some(t) = text.strip_prefix("@executable_path/") {
                    runtime.loader.parent().unwrap().join(t)
                } else {
                    PathBuf::from(text)
                }
            };
            let mut dependencies = Vec::new();
            let mut commands = macho.macho_load_commands()?;
            while let Some(command) = commands.next()? {
                if let LoadCommandVariant::Rpath(rpath) = command.variant()? {
                    rpaths.push(expand(std::str::from_utf8(
                        command.string(macho.endian(), rpath.path)?,
                    )?));
                }
                if command.cmd() != object::macho::LC_ID_DYLIB
                    && let Some(dylib) = command.dylib()?
                {
                    dependencies.push(
                        std::str::from_utf8(command.string(macho.endian(), dylib.dylib.name)?)?
                            .to_string(),
                    );
                }
            }
            for dependency in &dependencies {
                if dependency.starts_with("/usr/lib/") || dependency.starts_with("/System/Library/")
                {
                    continue;
                }
                let candidates = if let Some(name) = dependency.strip_prefix("@rpath/") {
                    rpaths.iter().map(|p| p.join(name)).collect::<Vec<_>>()
                } else {
                    vec![expand(dependency)]
                };
                ensure!(
                    candidates
                        .iter()
                        .any(|p| p.canonicalize().is_ok_and(|p| p.starts_with(root)
                            && crate::runtime_checks::native_arch(&p, runtime.arch()))),
                    "Native runtime dependency is missing or external: {} needs {dependency}. Bundle its matching native dependency before installing",
                    relative.display()
                );
            }
            result.insert(relative.clone(), dependencies);
        }
    }
    Ok(result)
}
fn check_runtime(root: &Path, files: &BTreeMap<PathBuf, Entry>) -> Result<ResolvedRuntime> {
    let runtime = ResolvedRuntime::inspect(root)?;
    ensure!(
        runtime.mode == ExecutionMode::Arm64Fex,
        "Managed FEX installation requires a native runtime descriptor"
    );
    validate_paths(files)?;
    // Inspect package-local components directly: global/backend environment
    // overrides must neither repair nor invalidate the imported package.
    let mut components = crate::launch::GraphicsComponents::default();
    if let Some(path) = &runtime.graphics.dxmt {
        let mut bundle =
            crate::dxmt::inspect_candidate_dir(path).context("Invalid bundled DXMT layout")?;
        bundle.x64_winemetal_so = Some(bundle.root.join(runtime.unix_name()).join("winemetal.so"));
        components.dxmt = Some(bundle);
    }
    if let Some(path) = &runtime.graphics.dxvk {
        components.dxvk = Some(
            crate::dxvk::inspect_candidate_dir(path).context("Invalid bundled DXVK/DXGI layout")?,
        );
    }
    if let Some(path) = &runtime.graphics.vkd3d {
        components.vkd3d = Some(
            crate::vkd3d::inspect_candidate_dir(path).context("Invalid bundled VKD3D layout")?,
        );
    }
    if let Some(manifest) = &runtime.graphics.vulkan_icd {
        let json: serde_json::Value = serde_json::from_slice(&fs::read(manifest)?)?;
        let library = manifest
            .parent()
            .unwrap()
            .join(
                json["ICD"]["library_path"]
                    .as_str()
                    .context("Invalid bundled ICD")?,
            )
            .canonicalize()?;
        components.vulkan.push(crate::launch::VulkanDriver {
            manifest: manifest.clone(),
            library,
        });
    }
    let is64 = runtime.guests.contains(&crate::runtime::GuestAbi::X64);
    let available = components.available(&runtime, &crate::detector::GraphicsApi::DirectX11, is64);
    ensure!(
        runtime.graphics.dxmt.is_none() || available.dxmt,
        "Bundled DXMT does not match the native runtime/guest ABI"
    );
    ensure!(
        runtime.graphics.vkd3d.is_none() || (available.vkd3d && available.vulkan),
        "Bundled VKD3D requires matching guest DLLs, DXGI, native Wine Vulkan, loader and driver"
    );
    ensure!(
        runtime.graphics.dxvk.is_none()
            || runtime.graphics.vkd3d.is_some()
            || (available.dxvk && available.vulkan),
        "Bundled DXVK requires matching guest DLLs and native Vulkan components"
    );
    ensure!(
        (runtime.graphics.vulkan_icd.is_none() && runtime.graphics.vulkan_loader.is_none())
            || available.vulkan,
        "Bundled Vulkan components are incomplete or incompatible"
    );
    Ok(runtime)
}

pub fn install(source: &Path) -> Result<PathBuf> {
    install_at(source, &paths::support_dir().join("runtimes/arm64-fex"))
}
fn install_at(source: &Path, store: &Path) -> Result<PathBuf> {
    install_impl(source, store, None)
}
fn install_impl(source: &Path, store: &Path, archive: Option<ArchiveOrigin>) -> Result<PathBuf> {
    let source = source.canonicalize()?;
    if source.join(RECEIPT).is_file() {
        verify(&source).context("Refusing to import a damaged managed generation")?;
    }
    let files = inventory(&source)?;
    let runtime = check_runtime(&source, &files)?;
    let dependencies = audit(&runtime, &files)?;
    let generation = staging::key(&(1, &files))?;
    ensure!(
        !std::path::absolute(store)?.starts_with(&source),
        "Managed runtime store cannot be inside the imported runtime"
    );
    fs::create_dir_all(store)?;
    let store = store.canonicalize()?;
    ensure!(
        !store.starts_with(&source),
        "Managed runtime store cannot be inside the imported runtime"
    );
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(store.join(".install.lock"))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;
    let mut target = store.join(&generation);
    let mut candidates = fs::read_dir(&store)?.collect::<std::io::Result<Vec<_>>>()?;
    candidates.sort_by_key(|entry| entry.file_name());
    for entry in candidates {
        if entry.file_type()?.is_dir()
            && entry.file_name().to_string_lossy().starts_with(&generation)
            && fs::read(entry.path().join(RECEIPT))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
                .is_some_and(|manifest| manifest.generation == generation)
            && verify(&entry.path()).is_ok()
        {
            return Ok(entry.path());
        }
    }
    if fs::symlink_metadata(&target).is_ok() {
        // Never replace mapped code, even when the old generation is damaged.
        let unique = tempfile::Builder::new()
            .prefix(&format!("{generation}-repair-"))
            .tempdir_in(&store)?;
        target = unique.path().to_path_buf();
        drop(unique);
    }
    let stage = tempfile::Builder::new()
        .prefix(".nucleon-runtime-")
        .tempdir_in(&store)?;
    for (relative, entry) in &files {
        let destination = stage.path().join(relative);
        match entry {
            Entry::Directory => fs::create_dir_all(destination)?,
            Entry::File { mode, .. } => {
                fs::create_dir_all(destination.parent().unwrap())?;
                fs::copy(source.join(relative), &destination)?;
                fs::set_permissions(destination, fs::Permissions::from_mode(*mode))?;
            }
            Entry::Link { .. } => {} // Create links after their target directories.
        }
    }
    for (relative, entry) in &files {
        if let Entry::Link { target } = entry {
            let destination = stage.path().join(relative);
            fs::create_dir_all(destination.parent().unwrap())?;
            std::os::unix::fs::symlink(target, destination)?;
        }
    }
    ensure!(
        inventory(stage.path())? == files,
        "Runtime changed during installation; previous generations were preserved"
    );
    let staged = check_runtime(stage.path(), &files)?;
    ensure!(
        audit(&staged, &files)? == dependencies,
        "Relocated native dependency closure changed"
    );
    let license_files = files
        .keys()
        .filter(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            name.contains("license") || name.starts_with("copying") || name.starts_with("notice")
        })
        .cloned()
        .collect();
    let identities = files
        .iter()
        .filter(|(_, entry)| matches!(entry, Entry::File { .. }))
        .map(|(relative, _)| {
            Ok((
                relative.clone(),
                staging::Identity::from(&fs::metadata(stage.path().join(relative))?),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let manifest = Manifest {
        schema: 1,
        generation,
        source,
        archive,
        files,
        identities,
        native_dependencies: dependencies,
        license_files,
        distribution: "local-import; redistribution rights are not granted by installation".into(),
    };
    atomic_write_file(
        stage.path().join(RECEIPT),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    // Under the store lock the destination is new. NOREPLACE also protects an
    // unexpected writer; a live generation is never exchanged or overwritten.
    crate::downloads::publish_new(stage.path(), &target)?;
    Ok(target)
}

/// Explicit full verification hashes every installed file and rechecks closure.
/// Ordinary launches fingerprint only their selected preparation inputs.
pub fn verify(root: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(root.join(RECEIPT)).context("Runtime is not a managed installation")?,
    )?;
    ensure!(
        manifest.schema == 1 && staging::key(&(1, &manifest.files))? == manifest.generation,
        "Invalid managed runtime manifest identity"
    );
    validate_paths(&manifest.files)?;
    ensure!(
        inventory(&root)? == manifest.files,
        "Managed runtime integrity check failed: {}. Install a verified source again to publish a separate repair generation",
        root.display()
    );
    let runtime = check_runtime(&root, &manifest.files)?;
    ensure!(
        audit(&runtime, &manifest.files)? == manifest.native_dependencies,
        "Native dependency closure changed"
    );
    Ok(())
}

/// Install-time hashes are reusable only while the installed file identity is
/// unchanged. Load a generation manifest once per process; verification remains
/// an explicit full hash pass rather than a per-launch operation.
pub(crate) fn known_digest(path: &Path, identity: &staging::Identity) -> Option<Result<String>> {
    use std::sync::{Mutex, OnceLock};
    static MANIFESTS: OnceLock<Mutex<BTreeMap<PathBuf, Manifest>>> = OnceLock::new();
    let root = path
        .ancestors()
        .skip(1)
        .find(|p| p.join(RECEIPT).is_file())?;
    let mut manifests = MANIFESTS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .ok()?;
    if !manifests.contains_key(root) {
        let manifest: Manifest =
            serde_json::from_slice(&fs::read(root.join(RECEIPT)).ok()?).ok()?;
        if manifest.schema != 1 {
            return None;
        }
        manifests.insert(root.to_path_buf(), manifest);
    }
    let manifest = manifests.get(root)?;
    let relative = path.strip_prefix(root).ok()?;
    if manifest.identities.get(relative) != Some(identity) {
        return None;
    }
    match manifest.files.get(relative)? {
        Entry::File { sha256, .. } => Some(Ok(sha256.clone())),
        _ => None,
    }
}

/// A shared generation lease outlives prefix cleanup. Installations never remove
/// old generations; the lease also provides the contract for future explicit GC.
pub fn lease(root: &Path) -> Result<Option<fs::File>> {
    if !root.join(RECEIPT).exists() {
        return Ok(None);
    }
    let locks = root
        .parent()
        .context("Managed runtime has no parent")?
        .join(".leases");
    fs::create_dir_all(&locks)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(locks.join(root.file_name().context("Managed runtime has no name")?))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockShared)?;
    Ok(Some(lock))
}

/// Import a checksum-pinned local archive using the same bounded extractor as
/// graphics downloads. Directory imports preserve their full provenance/notices.
pub fn install_archive(path: &Path, expected: &str) -> Result<PathBuf> {
    import_archive(fs::File::open(path)?, &path.to_string_lossy(), expected)
}

pub fn install_url(url: &str, expected: &str) -> Result<PathBuf> {
    ensure!(
        url.starts_with("https://"),
        "Runtime downloads require HTTPS"
    );
    validate_checksum(expected)?;
    import_archive(
        crate::downloads::download(url, 2 * 1024 * 1024 * 1024)?,
        url,
        expected,
    )
}
fn validate_checksum(expected: &str) -> Result<()> {
    ensure!(
        expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
        "Supply the archive's SHA256 checksum"
    );
    Ok(())
}
fn import_archive(reader: fs::File, name: &str, expected: &str) -> Result<PathBuf> {
    import_archive_to(
        reader,
        name,
        expected,
        &paths::support_dir().join("runtimes/arm64-fex"),
    )
}
fn import_archive_to(
    mut reader: fs::File,
    name: &str,
    expected: &str,
    store: &Path,
) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    validate_checksum(expected)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let size = reader.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    ensure!(
        format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected),
        "Runtime archive checksum mismatch"
    );
    reader.rewind()?;
    let name_path = name.split(['?', '#']).next().unwrap_or(name);
    let format = if name_path.ends_with(".tar.gz") {
        crate::downloads::ArchiveFormat::TarGz
    } else if name_path.ends_with(".tar.zst") {
        crate::downloads::ArchiveFormat::TarZstd
    } else if name_path.ends_with(".zip") {
        crate::downloads::ArchiveFormat::Zip
    } else {
        anyhow::bail!("Supported runtime archives: .tar.gz, .tar.zst, .zip")
    };
    let temp = tempfile::tempdir()?;
    crate::downloads::extract_with_limit(reader, format, temp.path(), MAX_BYTES)?;
    let roots = crate::fs_util::directories(temp.path(), 2)
        .filter(|p| p.join(crate::runtime::DESCRIPTOR).is_file())
        .collect::<Vec<_>>();
    ensure!(
        roots.len() == 1,
        "Archive must contain exactly one complete runtime"
    );
    install_impl(
        &roots[0],
        store,
        Some(ArchiveOrigin {
            location: name.into(),
            sha256: expected.to_ascii_lowercase(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installs_once_preserves_links_and_rejects_escaping_links() {
        let (_temp, runtime) = crate::runtime::fixture(ExecutionMode::Arm64Fex);
        fs::create_dir_all(runtime.root.join("share/licenses")).unwrap();
        fs::write(runtime.root.join("share/licenses/LICENSE"), b"notice").unwrap();
        std::os::unix::fs::symlink("wine", runtime.root.join("bin/wineboot")).unwrap();
        let destination = tempfile::tempdir().unwrap();
        let store = destination.path().join("store");
        let root = install_at(&runtime.root, &store).unwrap();
        assert_ne!(root, runtime.root);
        verify(&root).unwrap();
        assert_eq!(
            fs::read_link(root.join("bin/wineboot")).unwrap(),
            Path::new("wine")
        );
        assert_eq!(install_at(&runtime.root, &store).unwrap(), root);
        std::os::unix::fs::symlink("/etc/passwd", runtime.root.join("escape")).unwrap();
        assert!(install_at(&runtime.root, &store).is_err());
        verify(&root).unwrap();
    }
    #[test]
    fn damaged_live_generation_is_preserved_and_repair_is_published_separately() {
        let (_temp, runtime) = crate::runtime::fixture(ExecutionMode::Arm64Fex);
        let destination = tempfile::tempdir().unwrap();
        let store = destination.path().join("store");
        let root = install_at(&runtime.root, &store).unwrap();
        let _lease = lease(&root).unwrap();
        let image = root.join("bin/wine");
        assert_eq!(
            known_digest(
                &image,
                &staging::Identity::from(&fs::metadata(&image).unwrap())
            )
            .unwrap()
            .unwrap(),
            staging::digest(&image).unwrap()
        );
        fs::write(&image, b"damaged").unwrap();
        assert!(
            known_digest(
                &image,
                &staging::Identity::from(&fs::metadata(&image).unwrap())
            )
            .is_none()
        );
        assert!(verify(&root).is_err());
        let repair = install_at(&runtime.root, &store).unwrap();
        assert_ne!(root, repair);
        assert_eq!(fs::read(root.join("bin/wine")).unwrap(), b"damaged");
        verify(&repair).unwrap();
        assert_eq!(install_at(&runtime.root, &store).unwrap(), repair);
    }
    #[test]
    fn checksum_pinned_archives_reuse_the_shared_extractor_and_preserve_origin() {
        use std::io::{Seek, Write};
        let (_source, runtime) = crate::runtime::fixture(ExecutionMode::Arm64Fex);
        let destination = tempfile::tempdir().unwrap();
        let mut archive = tempfile::tempfile().unwrap();
        {
            let encoder = flate2::write::GzEncoder::new(&mut archive, flate2::Compression::fast());
            let mut tar = tar::Builder::new(encoder);
            tar.append_dir_all("runtime", &runtime.root).unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        archive.rewind().unwrap();
        let path = destination.path().join("runtime.tar.gz");
        let mut saved = fs::File::create(&path).unwrap();
        std::io::copy(&mut archive, &mut saved).unwrap();
        saved.flush().unwrap();
        let digest = staging::digest(&path).unwrap();
        assert!(
            import_archive_to(
                fs::File::open(&path).unwrap(),
                "runtime.tar.gz",
                &"0".repeat(64),
                &destination.path().join("bad-store")
            )
            .is_err()
        );
        assert!(!destination.path().join("bad-store").exists());
        let root = import_archive_to(
            fs::File::open(&path).unwrap(),
            "runtime.tar.gz",
            &digest,
            &destination.path().join("store"),
        )
        .unwrap();
        verify(&root).unwrap();
        let manifest: Manifest =
            serde_json::from_slice(&fs::read(root.join(RECEIPT)).unwrap()).unwrap();
        assert_eq!(manifest.archive.unwrap().sha256, digest);
    }
}
