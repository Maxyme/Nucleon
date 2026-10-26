//! Nucleon's native FSR build policy. Python runs only the pinned upstream tools.
use super::assets;
use crate::repo_assets::ASSETS;
use crate::{artifacts, fs_util::atomic_write_file};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const SOURCE_FILE_LIMIT: u64 = 64 * 1024 * 1024;
const BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const COMPILERS: &[&str] = &["dxc", "spirv-cross", "spirv-val"];
// These are calls into upstream's public build function, with no Nucleon build policy in Python.
const REFERENCE: &str =
    "import os, runpy, sys\nrunpy.run_path(sys.argv[1])[\"build_reference\"](os.environ)\n";

#[derive(Debug, Deserialize, Serialize)]
struct Source {
    commit: String,
    files: BTreeMap<PathBuf, String>,
}

fn pinned() -> Result<Source> {
    let source: Source = serde_json::from_slice(
        ASSETS
            .get_file("upscalers/fsr4-source.json")
            .context("Missing embedded FSR4 source lock")?
            .contents(),
    )?;
    ensure!(
        source.commit == assets::catalog().fsr_4.commit && !source.files.is_empty(),
        "Native FSR source lock differs from the dependency catalog"
    );
    for (path, sha) in &source.files {
        ensure!(
            safe_relative(path) && sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid native FSR source lock"
        );
    }
    Ok(source)
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path
            .as_os_str()
            .to_string_lossy()
            .contains(['\\', ':', '\0'])
        && path.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Create only real directories, including when multiple download workers share a parent.
fn directories(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure!(safe_relative(relative), "Unsafe source directory");
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_dir() && !metadata.is_symlink(),
            "Source directory is a file or symlink: {}",
            path.display()
        );
    }
    Ok(path)
}

fn blob(bytes: &[u8]) -> String {
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", bytes.len()));
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

fn source_bytes(root: &Path, relative: &Path, expected: &str) -> Result<Vec<u8>> {
    ensure!(safe_relative(relative), "Unsafe native FSR source path");
    let path = root.join(relative);
    ensure!(
        fs::symlink_metadata(&path)
            .with_context(|| format!("Read native FSR source {}", relative.display()))?
            .is_file()
            && path.canonicalize()?.starts_with(root),
        "Native FSR source escapes its root or is a symlink: {}",
        relative.display()
    );
    let file = fs::File::open(&path)?;
    ensure!(
        file.metadata()?.len() <= SOURCE_FILE_LIMIT,
        "Native FSR source file exceeds size limit"
    );
    let mut bytes = Vec::new();
    file.take(SOURCE_FILE_LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= SOURCE_FILE_LIMIT && blob(&bytes) == expected,
        "Native FSR source changed: {}",
        relative.display()
    );
    Ok(bytes)
}

fn fetch(cache: &Path, source: &Source) -> Result<PathBuf> {
    let catalog = assets::catalog();
    let repository = catalog.fsr_4.repository;
    cached_source(cache, source, &|relative| {
        let url = format!(
            "https://raw.githubusercontent.com/{repository}/{}/{}",
            source.commit,
            relative.display()
        );
        let mut file = artifacts::download(&url, SOURCE_FILE_LIMIT)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn cached_source(
    cache: &Path,
    source: &Source,
    download: &(impl Fn(&Path) -> Result<Vec<u8>> + Sync),
) -> Result<PathBuf> {
    let root = directories(
        cache,
        &Path::new("sources").join(format!("fsr4-{}", source.commit)),
    )?
    .canonicalize()?;
    let files = source.files.iter().collect::<Vec<_>>();
    std::thread::scope(|scope| -> Result<()> {
        let workers = files
            .chunks(files.len().div_ceil(8))
            .map(|chunk| {
                let root = &root;
                scope.spawn(move || -> Result<()> {
                    for (relative, expected) in chunk {
                        if source_bytes(root, relative, expected).is_ok() {
                            continue;
                        }
                        if let Some(parent) =
                            relative.parent().filter(|p| !p.as_os_str().is_empty())
                        {
                            directories(root, parent)?;
                        }
                        let bytes = download(relative)?;
                        ensure!(
                            blob(&bytes) == **expected,
                            "Native FSR download checksum mismatch: {}",
                            relative.display()
                        );
                        atomic_write_file(root.join(relative), bytes)?;
                    }
                    Ok(())
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("Native FSR download worker failed"))??;
        }
        Ok(())
    })?;
    atomic_write_file(
        root.join(".nucleon-source.json"),
        serde_json::to_vec_pretty(source)?,
    )?;
    Ok(root)
}

/// Copy only reviewed source files; user-supplied manifests and build products are never trusted.
fn copy_source(input: &Path, output: &Path, source: &Source) -> Result<()> {
    let input = input.canonicalize()?;
    fs::create_dir(output)?;
    for (relative, expected) in &source.files {
        let bytes = source_bytes(&input, relative, expected)?;
        if let Some(parent) = relative.parent().filter(|p| !p.as_os_str().is_empty()) {
            directories(output, parent)?;
        }
        atomic_write_file(output.join(relative), bytes)?;
    }
    atomic_write_file(
        output.join(".nucleon-source.json"),
        serde_json::to_vec_pretty(source)?,
    )
}

fn replace_once(text: &str, before: &str, after: &str) -> Result<String> {
    ensure!(
        text.matches(before).count() == 1,
        "Pinned FSR4 source no longer matches the compatibility adapter: {before}"
    );
    Ok(text.replacen(before, after, 1))
}

fn overlays(source: &Path) -> Result<PathBuf> {
    let overlay = directories(source, Path::new("nucleon"))?;
    let mut hook = fs::read_to_string(source.join("hook/fsr4_metalfx.m"))?;
    for (before, after) in [
        (
            "#import \"fsr4_live.h\"",
            "#import \"fsr4_live.h\"\n#import \"fsr4_depth_compat.h\"",
        ),
        (
            "    f->depthReversed = s.depthReversed ? 1 : 0;",
            "    const int capturedDepth = nucleon_depth_reversed(scaler);\n    if (capturedDepth < 0) { fsr4_live_set_inactive(\"depth direction unavailable\"); return 0; }\n    f->depthReversed = capturedDepth;",
        ),
        (
            "    if (!cls || orig_encode(cls)) return;",
            "    if (!cls || orig_encode(cls)) return;\n    nucleon_capture_depth_setter(cls);",
        ),
    ] {
        hook = replace_once(&hook, before, after)?;
    }
    atomic_write_file(overlay.join("fsr4_metalfx.m"), hook)?;
    for (name, contents) in [
        (
            "fsr4_depth_compat.h",
            ASSETS
                .get_file("upscalers/fsr4_depth_compat.h")
                .context("Missing embedded depth compatibility shim")?
                .contents(),
        ),
        (
            "test_depth_compat.m",
            ASSETS
                .get_file("upscalers/test_depth_compat.m")
                .context("Missing embedded depth compatibility test")?
                .contents(),
        ),
    ] {
        atomic_write_file(overlay.join(name), contents)?;
    }
    let panel = fs::read_to_string(source.join("src/fsr4_panel.mm"))?;
    let panel = replace_once(
        &panel,
        "    const char *root = getenv(\"FSR4_ROOT\");",
        "    const char *state = getenv(\"FSR4_STATE_DIR\");\n    if (state && state[0]) return std::string(state);\n    const char *root = getenv(\"FSR4_ROOT\");",
    )?;
    let panel = replace_once(
        &panel,
        "    const std::string app = directory + \"/FSR4 Panel.app\";",
        "    const std::string app = std::string(getenv(\"FSR4_ROOT\")) + \"/dist/crossover/FSR4 Panel.app\";",
    )?;
    let panel = replace_once(
        &panel,
        "{\"/usr/bin/open\", \"-g\", \"-a\",",
        "{\"/usr/bin/open\", \"-g\", \"-n\", \"-a\",",
    )?;
    atomic_write_file(overlay.join("fsr4_panel.mm"), panel)?;
    let app = fs::read_to_string(source.join("tools/crossover/panel_app.mm"))?;
    let app = replace_once(
        &app,
        "static std::string dist_path(const char *name) { return g_root + \"/dist/crossover/\" + name; }",
        "static std::string dist_path(const char *name) {\n    const char *state = getenv(\"FSR4_STATE_DIR\");\n    return state && state[0] ? std::string(state) + \"/\" + name : g_root + \"/dist/crossover/\" + name;\n}",
    )?;
    atomic_write_file(overlay.join("panel_app.mm"), app)?;

    // Preserve the pinned script. A separate copy binds its existing operations to
    // our source overlays and explicitly chosen framework; --no-bottle is mandatory.
    let mut builder = fs::read_to_string(source.join("tools/crossover/crossover_fsr4.py"))?;
    for (before, after) in [
        (
            "HOOK_SRC = ROOT / \"hook\" / \"fsr4_metalfx.m\"",
            "HOOK_SRC = ROOT / \"nucleon\" / \"fsr4_metalfx.m\"",
        ),
        (
            "PANEL_SRC = ROOT / \"src\" / \"fsr4_panel.mm\"",
            "PANEL_SRC = ROOT / \"nucleon\" / \"fsr4_panel.mm\"",
        ),
        (
            "PANEL_APP_SRC = ROOT / \"tools\" / \"crossover\" / \"panel_app.mm\"",
            "PANEL_APP_SRC = ROOT / \"nucleon\" / \"panel_app.mm\"",
        ),
        (
            "    framework = find_framework(args.crossover)",
            "    framework = Path(os.environ[\"NUCLEON_FSR4_FRAMEWORK\"])",
        ),
    ] {
        builder = replace_once(&builder, before, after)?;
    }
    let path = source.join("tools/crossover/nucleon_build.py");
    atomic_write_file(&path, builder)?;
    Ok(path)
}

#[derive(Debug, Serialize)]
struct Compiler {
    path: PathBuf,
    sha256: String,
}
fn compilers(source: &Path) -> Result<BTreeMap<String, Compiler>> {
    let bin = directories(source, Path::new("tools/bin"))?;
    let search = std::env::var_os("PATH").unwrap_or_default();
    let mut result = BTreeMap::new();
    for name in COMPILERS {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::split_paths(&search)
            .map(|dir| dir.join(name))
            .find(|path| {
                path.is_file()
                    && path
                        .metadata()
                        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
            })
            .with_context(|| format!("Missing shader compiler on PATH: {name}"))?
            .canonicalize()?;
        let sha256 = artifacts::digest(&path)?;
        std::os::unix::fs::symlink(&path, bin.join(name))?;
        result.insert((*name).into(), Compiler { path, sha256 });
    }
    Ok(result)
}

fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    crate::launch_profile::clean_profile_command(&mut command);
    command.stdin(Stdio::null());
    command
}

fn run(command: &mut Command, timeout: Duration) -> Result<()> {
    let description = command.get_program().to_string_lossy().into_owned();
    log::info!("Native FSR build: {description}");
    let mut child = crate::process::OwnedChild::spawn(command)
        .with_context(|| format!("Cannot start native FSR build tool {description}"))?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            child.terminate(Duration::ZERO)?;
            ensure!(
                status.success(),
                "Native FSR build tool {description} failed ({status})"
            );
            return Ok(());
        }
        ensure!(
            start.elapsed() < timeout,
            "Native FSR build tool {description} exceeded its deadline"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn environment(command: &mut Command, source: &Path, wine: &Path, profile: &str) {
    command
        .env(
            "NUCLEON_FSR4_FRAMEWORK",
            wine.join("lib/external/D3DMetal.framework"),
        )
        .env("FSR4_ROOT", source)
        .env("FSR4_REPLACE", "1")
        .env("FSR4_VERSION", "4.1.1")
        .env("FSR4_PROFILE", profile)
        .env("FSR4_MODEL", "auto")
        .env("FSR4_SHARPNESS", "0")
        .env("FSR4_SETTINGS", "none")
        .env("FSR4_PANEL", "0")
        .env("FSR4_HOOK_LOG", source.join("dist/crossover/hook-full.log"));
}

#[derive(Serialize)]
struct Validation {
    schema: u32,
    model: &'static str,
    profile: &'static str,
    synthetic_reference_and_live_hook: &'static str,
    profiles_validated: [&'static str; 2],
    game_tested: bool,
    original_framework_sha256: String,
    compilers: BTreeMap<String, Compiler>,
}

pub(super) fn build(root: &Path, output: &Path, wine: &Path, source: Option<&Path>) -> Result<()> {
    let pins = pinned()?;
    let input = match source {
        Some(path) => path.canonicalize()?,
        None => fetch(&directories(root, Path::new("cache"))?, &pins)?,
    };
    copy_source(&input, output, &pins)?;
    let tools = compilers(output)?;
    let builder = overlays(output)?;
    let test = output.join("nucleon/test-depth-compat");
    run(
        command("clang")
            .args(["-fobjc-arc", "-arch", "x86_64", "-framework", "Foundation"])
            .arg(output.join("nucleon/test_depth_compat.m"))
            .arg("-o")
            .arg(&test),
        BUILD_TIMEOUT,
    )?;
    run(
        command("/usr/bin/arch").arg("-x86_64").arg(&test),
        BUILD_TIMEOUT,
    )?;
    let original = wine.join("lib/external/D3DMetal.framework/D3DMetal");
    let original_framework_sha256 = artifacts::digest(&original)?;
    let mut install = command("python3");
    install.arg(&builder).args([
        "install",
        "--no-bottle",
        "--profile",
        "lossy",
        "--fsr-version",
        "4.1.1",
        "--panel",
    ]);
    environment(&mut install, output, wine, "lossy");
    run(&mut install, BUILD_TIMEOUT)?;
    let mut reference = command("python3");
    reference.arg("-c").arg(REFERENCE).arg(&builder);
    environment(&mut reference, output, wine, "exact-fast");
    run(&mut reference, BUILD_TIMEOUT)?;
    let dist = output.join("dist/crossover");
    let mut live = command("/usr/bin/arch");
    live.arg("-x86_64")
        .arg(dist.join("live_test"))
        .arg(output.join("testcases/testcase_quality_720to1080"))
        .arg(dist.join("reference-4.1.1-exact-fast-auto/f00_out.bin"));
    environment(&mut live, output, wine, "exact-fast");
    run(&mut live, BUILD_TIMEOUT)?;
    ensure!(
        artifacts::digest(&original)? == original_framework_sha256,
        "Original GPTK framework changed during build"
    );
    for (name, compiler) in &tools {
        ensure!(
            artifacts::digest(&compiler.path)? == compiler.sha256,
            "Shader compiler {name} changed during build"
        );
    }
    let report = Validation {
        schema: 1,
        model: "FSR 4.1.1 Lite",
        profile: "lossy",
        synthetic_reference_and_live_hook: "passed",
        profiles_validated: ["lossy", "exact-fast"],
        game_tested: false,
        original_framework_sha256,
        compilers: tools,
    };
    atomic_write_file(
        dist.join("nucleon-validation.json"),
        serde_json::to_vec_pretty(&report)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn fixture() -> Source {
        Source {
            commit: "329d1d7367b6b948a53486505198a5d254f02809".into(),
            files: [(PathBuf::from("src/reviewed.c"), blob(b"reviewed input"))].into(),
        }
    }

    #[test]
    fn pinned_sources_include_the_builder_models_and_notices_and_match_legacy_identity() {
        let pins = pinned().unwrap();
        assert_eq!(pins.files.len(), 395);
        for required in [
            "third_party/fsr411/LICENSE",
            "third_party/fsr411/NOTICE.upstream.md",
            "Makefile",
            "hook/fsr4_metalfx.m",
            "tools/crossover/crossover_fsr4.py",
            "tools/crossover/panel_app.mm",
            "src/fsr4_panel.mm",
            "third_party/fsr411/initializers.bin",
        ] {
            assert!(pins.files.contains_key(Path::new(required)), "{required}");
        }
        assert!(pins.files.keys().all(|p| !p.starts_with("tools/bin")));
        // Captured from native.py before retirement, using this exact framework input.
        assert_eq!(
            assets::native_key("framework-fixture").unwrap(),
            "7d256833b19c1cd25955625830c98692d525a1d04df899e1d7d915176231e4cc"
        );
        assert_eq!(blob(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(blob(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[test]
    fn cached_downloads_recover_without_replacing_input_with_failed_or_corrupt_downloads() {
        let temp = tempfile::tempdir().unwrap();
        let pins = fixture();
        let root = directories(
            temp.path(),
            &Path::new("sources").join(format!("fsr4-{}", pins.commit)),
        )
        .unwrap();
        directories(&root, Path::new("src")).unwrap();
        fs::write(root.join("src/reviewed.c"), b"old interrupted content").unwrap();
        assert!(
            cached_source(temp.path(), &pins, &|_| anyhow::bail!(
                "network interrupted"
            ))
            .is_err()
        );
        assert_eq!(
            fs::read(root.join("src/reviewed.c")).unwrap(),
            b"old interrupted content"
        );
        assert!(cached_source(temp.path(), &pins, &|_| Ok(b"wrong checksum".to_vec())).is_err());
        assert_eq!(
            fs::read(root.join("src/reviewed.c")).unwrap(),
            b"old interrupted content"
        );
        assert!(!root.join(".nucleon-source.json").exists());
        let repaired =
            cached_source(temp.path(), &pins, &|_| Ok(b"reviewed input".to_vec())).unwrap();
        assert_eq!(
            fs::read(repaired.join("src/reviewed.c")).unwrap(),
            b"reviewed input"
        );
        cached_source(temp.path(), &pins, &|_| {
            panic!("verified files must be reused")
        })
        .unwrap();
    }

    #[test]
    fn source_copy_checks_reviewed_hashes_ignores_untrusted_manifests_and_extra_build_products() {
        let temp = tempfile::tempdir().unwrap();
        let input = directories(temp.path(), Path::new("input/src"))
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        fs::write(input.join("src/reviewed.c"), b"reviewed input").unwrap();
        fs::write(input.join("src/unreviewed.c"), b"unreviewed extra").unwrap();
        fs::write(
            input.join(".nucleon-source.json"),
            br#"{"commit":"fake","files":{}}"#,
        )
        .unwrap();
        let output = temp.path().join("output");
        copy_source(&input, &output, &fixture()).unwrap();
        assert_eq!(
            fs::read(output.join("src/reviewed.c")).unwrap(),
            b"reviewed input"
        );
        assert!(!output.join("src/unreviewed.c").exists());
        let record: Source =
            serde_json::from_slice(&fs::read(output.join(".nucleon-source.json")).unwrap())
                .unwrap();
        assert_eq!(record.files, fixture().files);
        fs::write(input.join("src/reviewed.c"), b"changed input!").unwrap();
        assert!(
            copy_source(&input, &temp.path().join("failed"), &fixture())
                .unwrap_err()
                .to_string()
                .contains("source changed")
        );
    }

    #[test]
    fn source_links_and_cache_directory_links_cannot_escape() {
        let temp = tempfile::tempdir().unwrap();
        let root = directories(temp.path(), Path::new("source"))
            .unwrap()
            .canonicalize()
            .unwrap();
        directories(&root, Path::new("src")).unwrap();
        let outside = temp.path().join("outside.c");
        fs::write(&outside, b"reviewed input").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("src/reviewed.c")).unwrap();
        assert!(
            source_bytes(&root, Path::new("src/reviewed.c"), &blob(b"reviewed input")).is_err()
        );
        let cache = directories(temp.path(), Path::new("cache")).unwrap();
        std::os::unix::fs::symlink(&root, cache.join("sources")).unwrap();
        assert!(
            cached_source(&cache, &fixture(), &|_| panic!(
                "must reject before download"
            ))
            .is_err()
        );
        assert!(!root.join(format!("fsr4-{}", fixture().commit)).exists());
    }

    #[test]
    fn source_replacements_are_exact_and_fail_closed_on_missing_or_duplicate_context() {
        assert_eq!(
            replace_once("prefix NEEDLE suffix", "NEEDLE", "PATCH").unwrap(),
            "prefix PATCH suffix"
        );
        assert!(replace_once("missing", "NEEDLE", "PATCH").is_err());
        assert!(replace_once("NEEDLE and NEEDLE", "NEEDLE", "PATCH").is_err());
    }

    #[test]
    fn upstream_invocations_pin_synthetic_profiles_and_clear_parent_injection() {
        let mut cmd = command("python3");
        environment(
            &mut cmd,
            Path::new("/private/source"),
            Path::new("/private/wine"),
            "exact-fast",
        );
        let env = cmd
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for (key, value) in [
            ("FSR4_PROFILE", "exact-fast"),
            ("FSR4_PANEL", "0"),
            ("FSR4_VERSION", "4.1.1"),
            ("FSR4_MODEL", "auto"),
            ("FSR4_SETTINGS", "none"),
            (
                "NUCLEON_FSR4_FRAMEWORK",
                "/private/wine/lib/external/D3DMetal.framework",
            ),
        ] {
            assert_eq!(env.get(key).and_then(Option::as_deref), Some(value));
        }
        assert!(env.get("DYLD_INSERT_LIBRARIES").is_none_or(Option::is_none));
    }

    #[test]
    fn failing_or_timed_out_tools_cannot_report_success() {
        let status = run(
            command("/bin/sh").args(["-c", "exit 9"]),
            Duration::from_secs(5),
        )
        .unwrap_err()
        .to_string();
        assert!(
            status.contains("failed") && status.contains('9'),
            "{status}"
        );
        let temp = tempfile::tempdir().unwrap();
        let mut cmd = command("/bin/sh");
        cmd.args(["-c", "sleep 30 & echo $! > child; wait"])
            .current_dir(temp.path());
        let start = Instant::now();
        let error = run(&mut cmd, Duration::from_millis(200))
            .unwrap_err()
            .to_string();
        assert!(error.contains("deadline"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(5));
        let pid: u32 = fs::read_to_string(temp.path().join("child"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut system = sysinfo::System::new();
        let pid = sysinfo::Pid::from_u32(pid);
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        assert!(
            system
                .process(pid)
                .is_none_or(|p| p.status() == sysinfo::ProcessStatus::Zombie)
        );
    }

    proptest! {
        #[test]
        fn checked_patches_never_accept_ambiguous_context(before in "[a-zA-Z_]{1,32}", after in ".{0,80}") {
            let duplicate = format!("{before} {before}");
            prop_assert!(replace_once(&duplicate, &before, &after).is_err());
        }
    }

    #[test]
    #[ignore = "requires NUCLEON_TEST_FSR4_SOURCE pointing to an existing verified native provider"]
    fn real_pinned_source_produces_byte_identical_compatibility_overlays() {
        let input = PathBuf::from(std::env::var_os("NUCLEON_TEST_FSR4_SOURCE").unwrap());
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("fsr4");
        copy_source(&input, &output, &pinned().unwrap()).unwrap();
        overlays(&output).unwrap();
        for name in [
            "fsr4_metalfx.m",
            "fsr4_panel.mm",
            "panel_app.mm",
            "fsr4_depth_compat.h",
            "test_depth_compat.m",
        ] {
            assert_eq!(
                fs::read(output.join("nucleon").join(name)).unwrap(),
                fs::read(input.join("nucleon").join(name)).unwrap(),
                "{name}"
            );
        }
        assert_eq!(
            fs::read(output.join("tools/crossover/crossover_fsr4.py")).unwrap(),
            fs::read(input.join("tools/crossover/crossover_fsr4.py")).unwrap()
        );
    }
}
