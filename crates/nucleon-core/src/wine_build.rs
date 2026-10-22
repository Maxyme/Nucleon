#![forbid(unsafe_code)]

//! Download a reviewed Wine build or rebuild one matched PE module into a new
//! runtime copy. No global environment, original runtime, or active selection
//! is modified. Runtime artifacts retain Wine's LGPL license.
use crate::{
    artifacts::{copy_tree, digest},
    fs_util::atomic_write_file,
    paths, wine,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const RECOMMENDED_VERSION: &str = "11.18";
const RECIPE: &str = include_str!("../../../assets/wine/recipes/11.18.toml");
const ROSETTA_PATCH: &str =
    include_str!("../../../assets/patches/wine-11.18-wow64cpu-rosetta.patch");
const STEAM_PATCH: &str =
    include_str!("../../../assets/patches/wine-11.18-steamclient-redirect-experimental.patch");

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    version: String,
    binary_url: String,
    binary_sha256: String,
    source_url: String,
    source_sha256: String,
    core_files: BTreeMap<String, String>,
    module_files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WinePatch {
    Rosetta,
    SteamRedirect,
}

impl WinePatch {
    pub fn name(self) -> &'static str {
        match self {
            Self::Rosetta => "rosetta",
            Self::SteamRedirect => "steam-redirect",
        }
    }
    fn module(self) -> &'static str {
        match self {
            Self::Rosetta => "wow64cpu",
            Self::SteamRedirect => "ntdll",
        }
    }
    fn payload(self) -> &'static str {
        match self {
            Self::Rosetta => ROSETTA_PATCH,
            Self::SteamRedirect => STEAM_PATCH,
        }
    }
    fn relative_module(self) -> PathBuf {
        PathBuf::from(format!("lib/wine/x86_64-windows/{}.dll", self.module()))
    }
}

pub struct BuildOptions {
    pub runtime: PathBuf,
    pub output: PathBuf,
    pub patch: WinePatch,
    pub source_archive: Option<PathBuf>,
    pub jobs: usize,
}

pub fn default_runtime_path(version: &str, patch: Option<WinePatch>) -> PathBuf {
    let suffix = patch
        .map(|patch| format!("-{}", patch.name()))
        .unwrap_or_default();
    paths::support_dir()
        .join("wines")
        .join(format!("wine-devel-{version}{suffix}"))
}

fn recipe(version: &str) -> Result<Recipe> {
    ensure!(
        version == RECOMMENDED_VERSION,
        "Unsupported Wine {version}; reviewed recipes currently cover only {RECOMMENDED_VERSION}"
    );
    let result: Recipe = toml::from_str(RECIPE)?;
    ensure!(
        result.version == version,
        "Embedded Wine recipe version mismatch"
    );
    Ok(result)
}

fn require_macos() -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "Wine runtime installation/building requires macOS"
    );
    Ok(())
}

fn verify(path: &Path, expected: &str) -> Result<()> {
    ensure!(
        digest(path)? == expected,
        "Checksum mismatch: {}",
        path.display()
    );
    Ok(())
}

fn archive(
    version: &str,
    kind: &str,
    supplied: Option<&Path>,
    url: &str,
    expected: &str,
) -> Result<PathBuf> {
    if let Some(path) = supplied {
        let path = path.canonicalize()?;
        verify(&path, expected)?;
        return Ok(path);
    }
    let dir = paths::downloads_dir().join("wine").join(version);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{kind}.tar.xz"));
    if path.is_file() && verify(&path, expected).is_ok() {
        return Ok(path);
    }
    log::info!("Downloading reviewed Wine {kind} archive: {url}");
    let mut download = crate::artifacts::download(url, 2 * 1024 * 1024 * 1024)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&dir)?;
    std::io::copy(&mut download, &mut temporary)?;
    temporary.flush()?;
    verify(temporary.path(), expected)?;
    temporary
        .persist(&path)
        .map_err(|error| anyhow::anyhow!("Cache archive: {}", error.error))?;
    Ok(path)
}

fn clean_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    for (key, _) in std::env::vars_os() {
        let key_text = key.to_string_lossy();
        if key_text.starts_with("DYLD_") || key_text.starts_with("WINE") {
            command.env_remove(key);
        }
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    command.env(
        "PATH",
        format!(
            "/opt/homebrew/opt/bison/bin:/usr/local/opt/bison/bin:/opt/homebrew/bin:{}",
            path.to_string_lossy()
        ),
    );
    command
}

fn run(command: &mut Command, description: &str) -> Result<()> {
    log::info!("{description}");
    ensure!(
        command
            .status()
            .with_context(|| description.to_string())?
            .success(),
        "{description} failed"
    );
    Ok(())
}

fn output_location(output: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(output)?;
    ensure!(
        fs::symlink_metadata(&absolute).is_err(),
        "Output already exists: {}; choose a new runtime directory",
        absolute.display()
    );
    let parent = absolute.parent().context("Output has no parent")?;
    fs::create_dir_all(parent)?;
    Ok(parent
        .canonicalize()?
        .join(absolute.file_name().context("Output has no filename")?))
}

fn publish(staged: &Path, output: &Path) -> Result<()> {
    crate::downloads::publish_new(staged, output)
}

fn find_root(directory: &Path, depth: usize) -> Result<PathBuf> {
    if let Some(root) = wine::resolve_wine_runtime_root(directory) {
        return Ok(root);
    }
    if depth > 0 {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && let Ok(root) = find_root(&entry.path(), depth - 1)
            {
                return Ok(root);
            }
        }
    }
    bail!("No Wine runtime found in {}", directory.display())
}

fn verify_core(root: &Path, recipe: &Recipe) -> Result<()> {
    for (file, expected) in &recipe.core_files {
        verify(&root.join(file), expected).with_context(||
            "Runtime is not the reviewed Gcenx Wine base; matching --version alone is insufficient")?;
    }
    Ok(())
}

/// Preserve the complete downloaded package, including upstream resources/notices.
/// No runtime is selected or registered here; the CLI registers the returned root.
pub fn fetch(version: &str, output: &Path, supplied_archive: Option<&Path>) -> Result<PathBuf> {
    require_macos()?;
    let recipe = recipe(version)?;
    let output = output_location(output)?;
    let input = archive(
        version,
        "binary",
        supplied_archive,
        &recipe.binary_url,
        &recipe.binary_sha256,
    )?;
    let temporary = tempfile::Builder::new()
        .prefix(".nucleon-wine-fetch-")
        .tempdir_in(output.parent().unwrap())?;
    let package = temporary.path().join("package");
    fs::create_dir(&package)?;
    crate::artifacts::unpack(&input, &package, 8 * 1024 * 1024 * 1024)?;
    let root = find_root(&package, 3)?;
    verify_core(&root, &recipe)?;
    for (file, expected) in &recipe.module_files {
        verify(&root.join(file), expected)?;
    }
    let relative = root.strip_prefix(&package)?.to_path_buf();
    fs::write(
        root.join("nucleon-download.json"),
        serde_json::to_vec_pretty(&recipe)?,
    )?;
    publish(&package, &output)?;
    Ok(output.join(relative))
}

/// Create artifact directories without following symlinks inherited from Wine.
fn contained_directory(root: &Path, relative: &Path) -> Result<PathBuf> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            bail!("Build artifact path must stay inside the cloned runtime");
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "Build artifact directory is not a real directory: {}",
                current.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(current)
}

fn check_tools() -> Result<()> {
    let bison = clean_command("bison").arg("--version").output().context(
        "Install build prerequisites: Xcode Command Line Tools, mingw-w64, bison, flex, make",
    )?;
    let text = String::from_utf8_lossy(&bison.stdout);
    let major = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().last())
        .and_then(|version| version.split('.').next())
        .and_then(|major| major.parse::<u32>().ok());
    ensure!(
        bison.status.success() && major.is_some_and(|major| major >= 3),
        "Bison 3 or newer is required; install Homebrew bison"
    );
    for tool in [
        "clang",
        "clang++",
        "make",
        "flex",
        "patch",
        "x86_64-w64-mingw32-gcc",
    ] {
        let result = clean_command(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        ensure!(
            result.is_ok_and(|status| status.success()),
            "Missing build tool {tool}; install Xcode Command Line Tools and brew install mingw-w64 bison flex"
        );
    }
    Ok(())
}

/// Compile first, then copy Wine and replace exactly one matching PE module.
pub fn build(options: &BuildOptions) -> Result<PathBuf> {
    require_macos()?;
    ensure!(options.jobs > 0, "Build jobs must be positive");
    let recipe = recipe(RECOMMENDED_VERSION)?;
    let base = wine::resolve_wine_runtime_root(&options.runtime)
        .context("Input is not a Wine runtime")?
        .canonicalize()?;
    let output = output_location(&options.output)?;
    ensure!(
        !output.starts_with(&base),
        "Output must not be inside the input Wine runtime"
    );
    verify_core(&base, &recipe)?;
    let relative = options.patch.relative_module();
    let module_key = relative.to_str().context("Invalid module path")?;
    let expected = recipe
        .module_files
        .get(module_key)
        .context("No reviewed module fingerprint")?;
    verify(&base.join(&relative), expected)
        .context("Target module differs from the reviewed base or is already patched")?;
    check_tools()?;
    let input = archive(
        &recipe.version,
        "source",
        options.source_archive.as_deref(),
        &recipe.source_url,
        &recipe.source_sha256,
    )?;
    let work = tempfile::Builder::new()
        .prefix("nucleon-wine-build-")
        .tempdir()?;
    crate::artifacts::unpack(&input, work.path(), 8 * 1024 * 1024 * 1024)?;
    let source = work.path().join(format!("wine-{}", recipe.version));
    let patch_path = work.path().join("nucleon.patch");
    fs::write(&patch_path, options.patch.payload())?;
    run(
        clean_command("patch")
            .args(["--batch", "--fuzz=0", "-p1", "-i"])
            .arg(&patch_path)
            .current_dir(&source),
        "Apply reviewed Wine source patch",
    )?;
    let build = work.path().join("build");
    fs::create_dir(&build)?;
    let configure = [
        "--enable-archs=i386,x86_64",
        "--without-x",
        "--without-freetype",
        "--host=x86_64-apple-darwin",
        "CC=clang -arch x86_64",
        "CXX=clang++ -arch x86_64",
    ];
    run(
        clean_command(source.join("configure"))
            .args(configure)
            .current_dir(&build),
        "Configure Wine module build",
    )?;
    let target = format!(
        "dlls/{}/x86_64-windows/{}.dll",
        options.patch.module(),
        options.patch.module()
    );
    run(
        clean_command("make")
            .arg(format!("-j{}", options.jobs))
            .arg(&target)
            .current_dir(&build),
        "Compile patched Wine module",
    )?;
    let module = build.join(&target);
    let bytes = fs::read(&module)?;
    let image = object::File::parse(bytes.as_slice())?;
    ensure!(
        image.format() == object::BinaryFormat::Pe
            && object::Object::architecture(&image) == object::Architecture::X86_64,
        "Built module is not an x86_64 Windows PE"
    );
    // Refuse an input runtime changed while the compiler was working.
    verify_core(&base, &recipe)?;
    verify(&base.join(&relative), expected)?;
    log::info!("Copying Wine into a new runtime after successful compilation");
    let staging = tempfile::Builder::new()
        .prefix(".nucleon-wine-patch-")
        .tempdir_in(output.parent().unwrap())?;
    let runtime = staging.path().join("wine");
    copy_tree(&base, &runtime)?;
    verify_core(&runtime, &recipe)?;
    let destination = runtime.join(&relative);
    ensure!(
        destination
            .parent()
            .context("Missing module parent")?
            .canonicalize()?
            .starts_with(runtime.canonicalize()?),
        "Module directory escapes the cloned runtime through a symlink"
    );
    fs::remove_file(&destination)?; // unlink a DLL symlink rather than writing through it
    fs::copy(&module, &destination)?;
    let sources = contained_directory(&runtime, Path::new("share/nucleon-build"))?;
    atomic_write_file(
        sources.join("COPYING.LIB"),
        fs::read(source.join("COPYING.LIB"))?,
    )?;
    atomic_write_file(sources.join("AUTHORS"), fs::read(source.join("AUTHORS"))?)?;
    atomic_write_file(
        sources.join(format!("{}.patch", options.patch.name())),
        options.patch.payload(),
    )?;
    let receipt = serde_json::json!({
        "format": "nucleon-wine-build", "revision": 1, "wine": recipe.version,
        "base": base, "patch": options.patch, "source_url": recipe.source_url,
        "source_sha256": recipe.source_sha256, "binary_url": recipe.binary_url,
        "binary_sha256": recipe.binary_sha256, "source_archive": input,
        "patch_sha256": format!("{:x}", Sha256::digest(options.patch.payload().as_bytes())),
        "module": relative, "base_module_sha256": expected, "module_sha256": digest(&destination)?,
        "configure_arguments": configure, "make_target": target, "nucleon_version": env!("CARGO_PKG_VERSION"),
        "license": "LGPL-2.1-or-later", "experimental": matches!(options.patch, WinePatch::SteamRedirect),
        "activation": if matches!(options.patch, WinePatch::SteamRedirect) { Some("NUCLEON_STEAMCLIENT_REDIRECT=1") } else { None }
    });
    atomic_write_file(
        runtime.join(format!("nucleon-build-{}.json", options.patch.name())),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    verify_core(&base, &recipe)?;
    verify(&base.join(&relative), expected)?;
    publish(&runtime, &output)?;
    Ok(output)
}
