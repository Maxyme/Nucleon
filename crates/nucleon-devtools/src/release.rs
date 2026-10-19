//! Source-inclusive core releases. Publication requires both guest execution gates.
use crate::{
    build::{Build, RECIPE, marker_run},
    common::{
        clean_environment, copy, copy_tree, git, hash_files, json, key, p, patches, repo,
        require_native, run, set, sha, write_json,
    },
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub fn version_name(value: &str) -> Result<String> {
    let value = value.strip_prefix("runtime-arm64-fex-v").unwrap_or(value);
    let version =
        semver::Version::parse(value).context("Expected a runtime version such as 0.1.0-rc.1")?;
    ensure!(
        version.build.is_empty(),
        "Runtime versions cannot contain build metadata"
    );
    Ok(version.to_string())
}
pub fn checked_link(root: &Path, path: &Path) -> Result<PathBuf> {
    let link = fs::read_link(path)?;
    ensure!(
        !link.is_absolute() && path.canonicalize()?.starts_with(root.canonicalize()?),
        "Symlink escapes package: {}",
        path.display()
    );
    Ok(link)
}
pub fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .sort_by_file_name()
    {
        let entry = entry?;
        if !entry.file_type().is_dir() {
            files.push(entry.path().strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(files)
}
pub fn source_files(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let data = git(root, &["ls-files", "--stage", "-z"])?;
    let mut files = BTreeSet::new();
    for entry in data.split('\0').filter(|s| !s.is_empty()) {
        let (metadata, name) = entry.split_once('\t').context("Invalid Git index entry")?;
        let fields: Vec<_> = metadata.split_whitespace().collect();
        ensure!(
            fields.len() == 3 && fields[2] == "0",
            "Unmerged source path: {name}"
        );
        let path = root.join(name);
        if fields[0] == "160000" {
            if path.join(".git").exists() {
                ensure!(
                    git(&path, &["rev-parse", "HEAD"])? == fields[1],
                    "Submodule differs from source pin: {name}"
                );
                for child in source_files(&path)? {
                    files.insert(Path::new(name).join(child));
                }
            }
        } else {
            ensure!(
                path.is_file() || path.is_symlink(),
                "Missing source path: {name}"
            );
            files.insert(name.into());
        }
    }
    Ok(files)
}
pub fn archive(
    destination: &Path,
    root: &Path,
    names: &[PathBuf],
    prefix: &str,
    epoch: u64,
) -> Result<()> {
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let zipped = flate2::GzBuilder::new()
        .mtime(u32::try_from(epoch)?)
        .write(file, flate2::Compression::new(6));
    let mut archive = tar::Builder::new(zipped);
    archive.follow_symlinks(false);
    let mut names = names.to_vec();
    names.sort();
    for name in names {
        ensure!(
            name.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Invalid archive path"
        );
        let path = root.join(&name);
        let meta = fs::symlink_metadata(&path)?;
        ensure!(
            meta.permissions().mode() & 0o6000 == 0,
            "Privileged archive entry"
        );
        let mut header = tar::Header::new_gnu();
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(epoch);
        header.set_mode(meta.permissions().mode() & 0o777);
        let target = Path::new(prefix).join(name);
        if meta.is_symlink() {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            archive.append_link(&mut header, target, checked_link(root, &path)?)?;
        } else {
            ensure!(meta.is_file(), "Unsupported archive entry");
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(meta.len());
            archive.append_data(&mut header, target, fs::File::open(path)?)?;
        }
    }
    archive.into_inner()?.finish()?.sync_all()?;
    Ok(())
}
pub fn mark_changes(root: &Path, names: &[String], version: &str, date: &str) -> Result<()> {
    for name in names {
        let path = root.join(name);
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        ensure!(
            ["c", "cpp", "h", "m", "spec"].contains(&extension),
            "Unsupported modified-source notice: {name}"
        );
        let (prefix, end) = if extension == "spec" {
            ("# ", "")
        } else {
            ("/* ", " */")
        };
        let text = fs::read_to_string(&path)?;
        let text = if text.starts_with(&format!(
            "{prefix}Nucleon runtime downstream modifications:"
        )) {
            text.split_once('\n')
                .context("Truncated modification notice")?
                .1
        } else {
            &text
        };
        nucleon_core::fs_util::atomic_write_file(
            path,
            format!(
                "{prefix}Nucleon runtime downstream modifications: {date}, runtime {version}.{end}\n{text}"
            ),
        )?;
    }
    Ok(())
}
pub fn prepare_sources(build: &Build, version: &str) -> Result<()> {
    build.prepare_wine(&[])?;
    let date = git(&repo(), &["show", "-s", "--format=%cs", "HEAD"])?;
    for (root, patch_names) in [
        (&build.wine, &build.recipe.wine.tolkara.patches),
        (
            &build.fex,
            &build.recipe.alternatives["tolkara"]["fex"].unix_patches,
        ),
    ] {
        patches(root, patch_names)?;
        let names: Vec<_> = git(root, &["diff", "--name-only", "HEAD"])?
            .lines()
            .filter(|s| *s != build.recipe.wine.tolkara.shared_data_helper)
            .map(str::to_owned)
            .collect();
        mark_changes(root, &names, version, &date)?;
        patches(root, patch_names)?;
    }
    Ok(())
}
pub fn core_descriptor() -> String {
    // Keep a single runtime contract for the full x64+x86 core, with no graphics claims.
    include_str!("../../../assets/wine/runtime-arm64-fex.example.toml")
        .split("# Optional bundled components.")
        .next()
        .unwrap()
        .replace(
            "backends = [\"auto\", \"dxmt\", \"dxvk\", \"kosmickrisp\"]",
            "backends = [\"auto\"]",
        )
        .replace(
            "steam_protocol = \"valve-redirect\"",
            "steam_protocol = \"bridge\"",
        )
}
pub fn stage_core(build: &Build, target: &Path) -> Result<()> {
    let runtime = build.work.join("runtime");
    let receipt = json(&runtime.join("build.json"))?;
    ensure!(
        receipt["patches"]
            == serde_json::to_value(hash_files(&repo(), &build.recipe.wine.tolkara.patches)?)?
            && receipt["recipe_sha256"] == sha(&repo().join(RECIPE))?,
        "Wine build does not match release recipe; rebuild Wine"
    );
    for (name, digest) in receipt["host_files"]
        .as_object()
        .context("Missing build files")?
    {
        ensure!(
            sha(&runtime.join(name))? == digest.as_str().context("Invalid file digest")?,
            "Wine build receipt mismatch: {name}"
        );
    }
    fs::create_dir(target)?;
    for name in ["wine", "wineserver"] {
        copy(
            &runtime.join("bin").join(name),
            &target.join("bin").join(name),
        )?;
    }
    for name in [
        "lib/wine",
        "share/wine",
        "share/licenses",
        "share/nucleon-build",
    ] {
        if runtime.join(name).exists() {
            fs::create_dir_all(target.join(name).parent().unwrap())?;
            copy_tree(&runtime.join(name), &target.join(name))?;
        }
    }
    for name in files(target)? {
        if name.extension().is_some_and(|s| s == "a") {
            fs::remove_file(target.join(name))?;
        }
    }
    for (dir, name) in [
        ("arm64ec", "libarm64ecfex.dll"),
        ("wow64", "libwow64fex.dll"),
    ] {
        copy(
            &build.work.join("fex").join(dir).join("Bin").join(name),
            &target.join("lib/wine/aarch64-windows").join(name),
        )?;
    }
    let unix = build.work.join("fex/unixlib");
    let unix_receipt = json(&unix.join("build.json"))?;
    for (name, digest) in unix_receipt["files"]
        .as_object()
        .context("Missing FEX companion files")?
    {
        ensure!(
            sha(&unix.join(name))? == digest.as_str().context("Invalid FEX digest")?,
            "FEX companion receipt mismatch"
        );
        copy(
            &unix.join(name),
            &target.join("lib/wine/aarch64-unix").join(name),
        )?;
    }
    for (source, destination) in [
        (
            runtime.join("build.json"),
            "share/nucleon-build/wine-build.json",
        ),
        (
            unix.join("build.json"),
            "share/nucleon-build/fex-unix-build.json",
        ),
        (repo().join("NOTICE.md"), "share/nucleon-build/NOTICE.md"),
        (repo().join("LICENSE"), "share/licenses/nucleon/LICENSE"),
        (
            build.compiler.join("LICENSE.TXT"),
            "share/licenses/toolchain/LLVM-LICENSE.TXT",
        ),
    ] {
        copy(&source, &target.join(destination))?;
    }
    for triple in ["arm64ec", "aarch64", "i686"] {
        copy_tree(
            &build
                .compiler
                .join(format!("{triple}-w64-mingw32/share/mingw32")),
            &target.join("share/licenses/toolchain").join(triple),
        )?;
    }
    fs::write(target.join("nucleon-runtime.toml"), core_descriptor())?;
    let mut pe = Vec::new();
    let mut native = Vec::new();
    for name in files(target)? {
        let path = target.join(name);
        if path.is_symlink() {
            checked_link(target, &path)?;
            continue;
        }
        let mut magic = [0; 4];
        if fs::File::open(&path)?.read(&mut magic)? != 4 {
            continue;
        }
        if magic.starts_with(b"MZ") {
            pe.push(p(&path));
        } else if magic == [0xcf, 0xfa, 0xed, 0xfe] || magic == [0xfe, 0xed, 0xfa, 0xcf] {
            native.push(path);
        }
    }
    for batch in pe.chunks(128) {
        let mut args = vec!["--strip-debug".into()];
        args.extend_from_slice(batch);
        build.run(
            build.compiler.join("bin/llvm-strip"),
            &args,
            None,
            "release-strip.log",
        )?;
    }
    for path in native {
        build.run(
            "codesign",
            &[
                "--force".into(),
                "--sign".into(),
                "-".into(),
                "--timestamp=none".into(),
                p(&path),
            ],
            None,
            "release-sign.log",
        )?;
        build.run(
            "codesign",
            &["--verify".into(), "--strict".into(), p(path)],
            None,
            "release-sign.log",
        )?;
    }
    Ok(())
}
pub fn snapshot(build: &Build, stage: &Path) -> Result<Value> {
    snapshot_at(build, stage, &repo())
}
fn snapshot_at(build: &Build, stage: &Path, nucleon: &Path) -> Result<Value> {
    let mut manifest = BTreeMap::new();
    for (label, root, expected) in [
        (
            "wine",
            build.wine.clone(),
            Some(
                build.recipe.alternatives["tolkara"]["wine"]
                    .revision
                    .clone(),
            ),
        ),
        (
            "fex",
            build.fex.clone(),
            Some(build.recipe.alternatives["tolkara"]["fex"].revision.clone()),
        ),
        (
            "shared-data-origin",
            build.sources.join("wine"),
            Some(build.recipe.sources["wine"].revision.clone()),
        ),
        ("nucleon", nucleon.to_path_buf(), None),
    ] {
        let revision = git(&root, &["rev-parse", "HEAD"])?;
        if let Some(expected) = expected {
            ensure!(revision == expected, "Unexpected source revision: {label}");
        }
        let mut names = source_files(&root)?;
        if label == "nucleon" {
            // Include this implementation before it is committed, without sweeping arbitrary untracked files.
            names.extend(
                [
                    "Cargo.toml",
                    "Cargo.lock",
                    "crates/nucleon-devtools/Cargo.toml",
                    "crates/nucleon-core/src/artifacts.rs",
                    "crates/nucleon-core/src/runtime_selection.rs",
                    "tools/runtime-probes/fex-unixlib.c",
                ]
                .map(PathBuf::from),
            );
            names.extend(
                files(&root.join("crates/nucleon-devtools/src"))?
                    .into_iter()
                    .filter(|p| p.extension().is_some_and(|s| s == "rs"))
                    .map(|p| Path::new("crates/nucleon-devtools/src").join(p)),
            );
        }
        let destination = stage.join("sources").join(label);
        let mut digests = BTreeMap::new();
        for name in &names {
            let source = root.join(name);
            let target = destination.join(name);
            fs::create_dir_all(target.parent().unwrap())?;
            let value = if source.is_symlink() {
                let link = checked_link(&root, &source)?;
                std::os::unix::fs::symlink(&link, &target)?;
                json!({"link":link})
            } else {
                copy(&source, &target)?;
                json!({"sha256":sha(&target)?})
            };
            digests.insert(p(name), value);
        }
        for name in &names {
            if destination.join(name).is_symlink() {
                checked_link(&destination, &destination.join(name))?;
            }
        }
        manifest.insert(label,json!({"revision":revision,"downstream_modified_paths":git(&root,&["diff","--name-only","HEAD"])?.lines().collect::<Vec<_>>(),"files":digests}));
    }
    let value = serde_json::to_value(manifest)?;
    write_json(&stage.join("SOURCE-MANIFEST.json"), &value)?;
    Ok(value)
}
pub fn build_identity(build: &Build, source_manifest: &Value) -> Result<Value> {
    let runtime = build.work.join("runtime");
    let mut products = BTreeMap::new();
    for name in [
        "bin/wine",
        "bin/wineserver",
        "lib/wine",
        "share/wine",
        "share/licenses",
        "share/nucleon-build",
        "build.json",
    ] {
        let path = runtime.join(name);
        let names = if path.is_dir() {
            files(&path)?
                .into_iter()
                .map(|p| Path::new(name).join(p))
                .collect()
        } else {
            vec![name.into()]
        };
        for relative in names {
            if relative.extension().is_some_and(|s| s == "a") {
                continue;
            }
            let path = runtime.join(&relative);
            if path.is_symlink() {
                products.insert(p(&relative), json!({"link":checked_link(&runtime,&path)?}));
            } else if path.is_file() {
                products.insert(p(&relative),json!({"sha256":sha(&path)?,"mode":path.metadata()?.permissions().mode()&0o777}));
            }
        }
    }
    for (dir, name) in [
        ("arm64ec", "libarm64ecfex.dll"),
        ("wow64", "libwow64fex.dll"),
        ("unixlib", "libarm64ecfex.so"),
        ("unixlib", "libwow64fex.so"),
    ] {
        let file = build.work.join("fex").join(dir).join(if dir == "unixlib" {
            PathBuf::from(name)
        } else {
            Path::new("Bin").join(name)
        });
        products.insert(format!("fex/{name}"), json!(sha(&file)?));
    }
    let mut compiled = source_manifest
        .as_object()
        .context("Invalid source manifest")?
        .clone();
    compiled.remove("nucleon");
    let inputs: BTreeMap<_, _> = source_manifest["nucleon"]["files"]
        .as_object()
        .context("Missing build inputs")?
        .iter()
        .filter(|(k, _)| {
            k.starts_with("assets/wine/recipes/")
                || k.starts_with("assets/patches/")
                || k.starts_with("crates/nucleon-devtools/")
                || matches!(k.as_str(), "Cargo.toml" | "Cargo.lock")
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    compiled.insert("build-scripts".into(), serde_json::to_value(inputs)?);
    Ok(json!({"schema":2,"compiled_sources_sha256":key(&compiled)?,"files":products}))
}
fn validate_install(build: &Build, archive: &Path, work: &Path, logs: &Path) -> Result<Value> {
    let runtime = nucleon_core::managed_runtime::install_archive_at(
        archive,
        &sha(archive)?,
        &work.join("runtimes/arm64-fex"),
    )?;
    nucleon_core::managed_runtime::verify(&runtime)?;
    let descriptor = nucleon_core::runtime::ResolvedRuntime::inspect(&runtime)?;
    let mut env = clean_environment();
    for (k, v) in &descriptor.environment {
        set(&mut env, k, v);
    }
    for (k, v) in [
        ("WINEPREFIX", p(work.join("smoke-prefix"))),
        ("WINELOADER", p(&descriptor.loader)),
        ("WINESERVER", p(&descriptor.server)),
        (
            "WINEDLLPATH",
            p(runtime.join("lib/wine/aarch64-windows"))
                + ":"
                + &p(runtime.join("lib/wine/i386-windows")),
        ),
        ("DYLD_LIBRARY_PATH", p(&descriptor.unix_dir)),
        ("DYLD_FALLBACK_LIBRARY_PATH", p(runtime.join("lib"))),
        ("WINEDEBUG", "-all,warn+all,err+all".into()),
        (
            "WINEDLLOVERRIDES",
            "winemenubuilder.exe=d;mscoree=d;mshtml=d".into(),
        ),
    ] {
        set(&mut env, k, v);
    }
    let mut validation = json!({"schema":2,"managed_archive_import":"PASSED","integrity_and_native_dependencies":"PASSED","guest_smoke":{},"steam":"NOT_RUN","gameplay":"NOT_RUN","status":"TESTING"});
    write_json(&logs.join("validation.json"), &validation)?;
    let result = (|| {
        run(
            &descriptor.loader,
            &["wineboot".into(), "-i".into()],
            Some(&env),
            None,
            Some(&logs.join("wineboot.log")),
            120,
            0,
        )?;
        for (guest, triple, bits) in [("x64", "x86_64", 64), ("x86", "i686", 32)] {
            validation["guest_smoke"][guest] = "TESTING".into();
            write_json(&logs.join("validation.json"), &validation)?;
            let exe = logs.join(format!("smoke-{guest}.exe"));
            build.run(
                build
                    .compiler
                    .join(format!("bin/{triple}-w64-mingw32-clang")),
                &[
                    "-O2".into(),
                    "-municode".into(),
                    "-fms-extensions".into(),
                    p(repo().join("tools/runtime-probes/windows.c")),
                    "-o".into(),
                    p(&exe),
                ],
                None,
                &format!("compile-{guest}.log"),
            )?;
            let log = logs.join(format!("smoke-{guest}.log"));
            marker_run(
                &descriptor.loader,
                &[p(exe)],
                &env,
                None,
                &log,
                60,
                0,
                "NUCLEON_FEX_SMOKE_OK",
            )?;
            ensure!(
                fs::read_to_string(log)?
                    .contains(&format!("NUCLEON_FEX_BEGIN pointer_bits={bits}")),
                "Missing guest ABI marker: {guest}"
            );
            validation["guest_smoke"][guest] = "PASSED".into();
            write_json(&logs.join("validation.json"), &validation)?;
        }
        Ok(())
    })();
    let wait = crate::common::cleanup_server(&descriptor.server, &env, &logs.join("cleanup.log"));
    validation["prefix_cleanup"] = if wait.is_ok() { "PASSED" } else { "FAILED" }.into();
    let result = result.and(wait);
    if let Err(error) = &result {
        for status in validation["guest_smoke"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            if status == "TESTING" {
                *status = "FAILED".into();
            }
        }
        validation["status"] = "FAILED".into();
        validation["error"] = format!("{error:#}").into();
    } else {
        validation["status"] = "PASSED".into();
    }
    write_json(&logs.join("validation.json"), &validation)?;
    result?;
    Ok(validation)
}
/// Exercise the same archive/import/execution gates independently of publication.
pub fn validate_built(build: &Build, logs: &Path) -> Result<()> {
    fs::create_dir(logs).context("Validation output must be a new directory")?;
    let temp = tempfile::tempdir_in(logs)?;
    let runtime = temp.path().join("runtime");
    stage_core(build, &runtime)?;
    let binary = temp.path().join("runtime.tar.gz");
    archive(&binary, &runtime, &files(&runtime)?, "", 0)?;
    validate_install(build, &binary, &temp.path().join("support"), logs)?;
    Ok(())
}
pub fn publish_assets(target: &Path, build: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    ensure!(
        !target.exists(),
        "Output already exists; preserve existing assets"
    );
    let parent = target.parent().context("Output has no parent")?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".runtime-release-")
        .tempdir_in(parent)?;
    let assets = stage.path().join("assets");
    fs::create_dir(&assets)?;
    build(&assets)?;
    nucleon_core::artifacts::publish_new(&assets, target)
}
pub fn package(build: &Build, version: &str, target: &Path) -> Result<()> {
    let version = version_name(version)?;
    let epoch = git(&repo(), &["show", "-s", "--format=%ct", "HEAD"])?.parse()?;
    let logs = build.work.join("release-validation").join(format!(
        "{}-{}",
        version,
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::create_dir_all(&logs)?;
    publish_assets(target, |assets| {
        let stage = tempfile::tempdir_in(assets.parent().unwrap())?;
        let runtime = stage.path().join("runtime");
        stage_core(build, &runtime)?;
        let manifest = snapshot(build, stage.path())?;
        ensure!(
            json(&build.work.join("publisher-build-rust.json"))?
                == build_identity(build, &manifest)?,
            "Sources/products changed since publisher build; run release build again"
        );
        let binary = format!("nucleon-arm64-fex-core-{version}-macos-arm64.tar.gz");
        let source = format!("nucleon-arm64-fex-core-{version}-sources.tar.gz");
        let mut release = json!({"schema":2,"version":version,"channel":"experimental","component":"wine-fex-core","guests":["x64","x86"],"graphics":"WineD3D; graphics add-ons distributed separately","postprocessing":["PE llvm-strip --strip-debug","Mach-O ad-hoc signing; no Mach-O stripping"],"toolchain":build.recipe.toolchain,"source_manifest_sha256":sha(&stage.path().join("SOURCE-MANIFEST.json"))?});
        write_json(&runtime.join("share/nucleon-build/release.json"), &release)?;
        archive(
            &assets.join(&binary),
            &runtime,
            &files(&runtime)?,
            "",
            epoch,
        )?;
        let mut sources = files(&stage.path().join("sources"))?
            .into_iter()
            .map(|p| Path::new("sources").join(p))
            .collect::<Vec<_>>();
        sources.push("SOURCE-MANIFEST.json".into());
        archive(&assets.join(&source), stage.path(), &sources, "", epoch)?;
        release["validation"] = validate_install(
            build,
            &assets.join(&binary),
            &stage.path().join("support"),
            &logs,
        )?;
        release["binary"] = json!({"file":binary,"sha256":sha(&assets.join(&binary))?});
        release["sources"] = json!({"file":source,"sha256":sha(&assets.join(&source))?});
        write_json(&assets.join("runtime-release.json"), &release)?;
        fs::write(
            assets.join("RELEASE-NOTES.md"),
            format!(
                "# Experimental ARM64 Wine/FEX core {version}\n\nCPU/core preview. No DXMT, DXVK/VKD3D, Vulkan, Steam bridge or game compatibility certification.\n\n```sh\nnucleon wine fex install ./{binary} --sha256 {} --register\nnucleon wine fex verify\n```\n\nCorresponding sources and rebuild instructions: {source}; sources/nucleon/RUNTIME-RELEASES.md.\n",
                sha(&assets.join(&binary))?
            ),
        )?;
        let mut checksums = String::new();
        for name in files(assets)? {
            checksums.push_str(&format!(
                "{}  {}\n",
                sha(&assets.join(&name))?,
                name.display()
            ));
        }
        fs::write(assets.join("SHA256SUMS.txt"), checksums)?;
        copy_tree(&logs, &assets.join("validation"))?;
        Ok(())
    })
}
pub fn build_release(build: &Build, version: &str, target: &Path) -> Result<()> {
    require_native()?;
    let version = version_name(version)?;
    ensure!(!target.exists(), "Output must not exist");
    build.fetch(false)?;
    build.doctor()?;
    prepare_sources(build, &version)?;
    build.build_fex()?;
    let runtime = build.work.join("runtime");
    if runtime.exists() {
        let retained = tempfile::Builder::new()
            .prefix("retained-runtime-")
            .tempdir_in(&build.work)?
            .keep();
        fs::rename(&runtime, retained.join("runtime"))?;
    }
    build.build_wine(&[], None)?;
    let sources = tempfile::tempdir_in(&build.work)?;
    let manifest = snapshot(build, sources.path())?;
    write_json(
        &build.work.join("publisher-build-rust.json"),
        &build_identity(build, &manifest)?,
    )?;
    package(build, &version, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_repo(root: &Path) -> String {
        fs::create_dir_all(root).unwrap();
        git(root, &["init", "-q"]).unwrap();
        fs::write(root.join("tracked.c"), "original\n").unwrap();
        git(root, &["add", "tracked.c"]).unwrap();
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "fixture",
            ],
        )
        .unwrap();
        git(root, &["rev-parse", "HEAD"]).unwrap()
    }
    #[test]
    fn submodules_are_pinned_and_snapshot_bytes_are_immutable() {
        let dir = tempfile::tempdir().unwrap();
        let mut build = Build::new(dir.path(), 1).unwrap();
        for (root, key) in [(&build.wine, "wine"), (&build.fex, "fex")] {
            let revision = fixture_repo(root);
            build
                .recipe
                .alternatives
                .get_mut("tolkara")
                .unwrap()
                .get_mut(key)
                .unwrap()
                .revision = revision;
        }
        build.recipe.sources.get_mut("wine").unwrap().revision =
            fixture_repo(&build.sources.join("wine"));
        let dependency = dir.path().join("dependency");
        fixture_repo(&dependency);
        git(
            &build.fex,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                dependency.to_str().unwrap(),
                "External/dependency",
            ],
        )
        .unwrap();
        assert!(
            source_files(&build.fex)
                .unwrap()
                .contains(Path::new("External/dependency/tracked.c"))
        );
        let nucleon = dir.path().join("nucleon");
        fixture_repo(&nucleon);
        for name in [
            "Cargo.toml",
            "Cargo.lock",
            "crates/nucleon-devtools/Cargo.toml",
            "crates/nucleon-devtools/src/main.rs",
            "crates/nucleon-core/src/artifacts.rs",
            "crates/nucleon-core/src/runtime_selection.rs",
            "tools/runtime-probes/fex-unixlib.c",
        ] {
            let path = nucleon.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "publisher source").unwrap();
        }
        fs::write(build.wine.join("tracked.c"), "patched\n").unwrap();
        fs::write(build.wine.join("helper.h"), "borrowed helper\n").unwrap();
        git(&build.wine, &["add", "--intent-to-add", "helper.h"]).unwrap();
        fs::write(build.wine.join("untracked-secret"), "must not ship").unwrap();
        let snapshot = dir.path().join("snapshot");
        let manifest = snapshot_at(&build, &snapshot, &nucleon).unwrap();
        fs::write(build.wine.join("tracked.c"), "changed after snapshot").unwrap();
        assert_eq!(
            fs::read(snapshot.join("sources/wine/tracked.c")).unwrap(),
            b"patched\n"
        );
        assert_eq!(
            manifest["wine"]["files"]["tracked.c"]["sha256"],
            sha(&snapshot.join("sources/wine/tracked.c")).unwrap()
        );
        assert!(!snapshot.join("sources/wine/untracked-secret").exists());
        assert!(
            snapshot
                .join("sources/fex/External/dependency/tracked.c")
                .is_file()
        );
        assert!(
            snapshot
                .join("sources/nucleon/crates/nucleon-core/src/artifacts.rs")
                .is_file()
        );
    }
    #[test]
    fn build_identity_covers_guest_dlls_and_both_native_companions() {
        let dir = tempfile::tempdir().unwrap();
        let build = Build::new(dir.path(), 1).unwrap();
        let names = [
            "runtime/bin/wine",
            "runtime/lib/wine/i386-windows/user32.dll",
            "fex/arm64ec/Bin/libarm64ecfex.dll",
            "fex/wow64/Bin/libwow64fex.dll",
            "fex/unixlib/libarm64ecfex.so",
            "fex/unixlib/libwow64fex.so",
        ];
        for name in names {
            let path = build.work.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"original").unwrap();
        }
        let sources = json!({"wine":{"files":{}},"nucleon":{"files":{}}});
        let before = build_identity(&build, &sources).unwrap();
        for name in [names[1], names[4], names[5]] {
            fs::write(build.work.join(name), b"changed").unwrap();
            assert_ne!(before, build_identity(&build, &sources).unwrap());
            fs::write(build.work.join(name), b"original").unwrap();
        }
    }
    #[test]
    fn core_packaging_strips_only_pe_and_preserves_native_macho_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut build = Build::new(dir.path(), 1).unwrap();
        let runtime = build.work.join("runtime");
        let native = b"\xcf\xfa\xed\xfenative layout";
        let names = [
            "bin/wine",
            "bin/wineserver",
            "lib/wine/aarch64-unix/ntdll.so",
        ]
        .map(str::to_owned);
        for name in &names {
            let path = runtime.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, native).unwrap();
        }
        write_json(&runtime.join("build.json"),&json!({"patches":hash_files(&repo(),&build.recipe.wine.tolkara.patches).unwrap(),"recipe_sha256":sha(&repo().join(RECIPE)).unwrap(),"host_files":hash_files(&runtime,&names).unwrap()})).unwrap();
        for (sub, name) in [
            ("arm64ec/Bin", "libarm64ecfex.dll"),
            ("wow64/Bin", "libwow64fex.dll"),
            ("unixlib", "libarm64ecfex.so"),
            ("unixlib", "libwow64fex.so"),
        ] {
            let path = build.work.join("fex").join(sub).join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path,
                if name.ends_with("dll") {
                    b"MZ guest".as_slice()
                } else {
                    native
                },
            )
            .unwrap();
        }
        let unix = build.work.join("fex/unixlib");
        write_json(&unix.join("build.json"),&json!({"files":hash_files(&unix,&["libarm64ecfex.so".into(),"libwow64fex.so".into()]).unwrap()})).unwrap();
        for triple in ["arm64ec", "aarch64", "i686"] {
            let path = build
                .compiler
                .join(format!("{triple}-w64-mingw32/share/mingw32"));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("COPYING"), "notice").unwrap();
        }
        fs::write(build.compiler.join("LICENSE.TXT"), "compiler notice").unwrap();
        let bin = dir.path().join("fake-bin");
        fs::create_dir(&bin).unwrap();
        fs::create_dir_all(build.compiler.join("bin")).unwrap();
        for path in [bin.join("codesign"), build.compiler.join("bin/llvm-strip")] {
            fs::write(
                &path,
                "#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" >> \"$TOOL_CAPTURE\"\n",
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let capture = dir.path().join("commands");
        set(&mut build.env, "PATH", &bin);
        set(&mut build.env, "TOOL_CAPTURE", &capture);
        let target = dir.path().join("package");
        stage_core(&build, &target).unwrap();
        assert_eq!(fs::read(target.join("bin/wine")).unwrap(), native);
        assert!(
            target
                .join("share/licenses/toolchain/LLVM-LICENSE.TXT")
                .is_file()
        );
        let commands = fs::read_to_string(capture).unwrap();
        let strip = commands
            .split("--strip-debug\n")
            .nth(1)
            .unwrap()
            .split("codesign")
            .next()
            .unwrap();
        assert!(strip.contains("libarm64ecfex.dll"));
        assert!(
            !strip
                .lines()
                .any(|l| l.ends_with("/bin/wine") || l.ends_with(".so"))
        );
    }
    #[test]
    fn versions_notices_and_failed_publication_preserve_contracts() {
        assert_eq!(
            version_name("runtime-arm64-fex-v0.1.0-rc.1").unwrap(),
            "0.1.0-rc.1"
        );
        for value in [
            "v1.2.3",
            "../../bad",
            "1.0",
            "1.0.0; echo oops",
            "1.0.0\n",
            "1.0.0+metadata",
        ] {
            assert!(version_name(value).is_err());
        }
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("assets");
        assert!(
            publish_assets(&target, |p| {
                fs::write(p.join("binary"), b"partial")?;
                anyhow::bail!("guest failed")
            })
            .is_err()
        );
        assert!(!target.exists());
        fs::write(dir.path().join("module.c"), "/* upstream */\n").unwrap();
        for version in ["0.1.0-rc.1", "0.1.0-rc.2"] {
            mark_changes(dir.path(), &["module.c".into()], version, "2026-10-18").unwrap();
        }
        let text = fs::read_to_string(dir.path().join("module.c")).unwrap();
        assert_eq!(
            text.matches("Nucleon runtime downstream modifications:")
                .count(),
            1
        );
        assert!(text.contains("/* upstream */"));
        let descriptor: toml::Value = toml::from_str(&core_descriptor()).unwrap();
        assert_eq!(descriptor["guests"].as_array().unwrap().len(), 2);
        assert!(descriptor.get("graphics").is_none());
    }
    #[test]
    fn archive_preserves_modes_links_and_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("wine"), b"native").unwrap();
        fs::set_permissions(root.join("wine"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("wine", root.join("wineboot")).unwrap();
        for name in ["a.tar.gz", "b.tar.gz"] {
            archive(
                &dir.path().join(name),
                &root,
                &files(&root).unwrap(),
                "",
                123,
            )
            .unwrap();
        }
        assert_eq!(
            fs::read(dir.path().join("a.tar.gz")).unwrap(),
            fs::read(dir.path().join("b.tar.gz")).unwrap()
        );
        let mut reader = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(dir.path().join("a.tar.gz")).unwrap(),
        ));
        for entry in reader.entries().unwrap() {
            let e = entry.unwrap();
            assert_eq!(e.header().uid().unwrap(), 0);
            assert_eq!(e.header().mtime().unwrap(), 123);
            if e.path().unwrap() == Path::new("wine") {
                assert_eq!(e.header().mode().unwrap(), 0o755);
            } else {
                assert_eq!(e.link_name().unwrap().unwrap(), Path::new("wine"));
            }
        }
        std::os::unix::fs::symlink("../a.tar.gz", root.join("escape")).unwrap();
        assert!(checked_link(&root, &root.join("escape")).is_err());
        std::os::unix::fs::symlink("missing", root.join("dangling")).unwrap();
        assert!(checked_link(&root, &root.join("dangling")).is_err());
    }
    #[test]
    fn source_export_tracks_patched_and_intent_added_bytes_not_untracked_files() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]).unwrap();
        fs::write(dir.path().join("tracked.c"), "original").unwrap();
        git(dir.path(), &["add", "tracked.c"]).unwrap();
        git(
            dir.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "fixture",
            ],
        )
        .unwrap();
        fs::write(dir.path().join("tracked.c"), "patched").unwrap();
        fs::write(dir.path().join("helper.h"), "helper").unwrap();
        git(dir.path(), &["add", "--intent-to-add", "helper.h"]).unwrap();
        fs::write(dir.path().join("secret"), "private").unwrap();
        assert_eq!(
            source_files(dir.path()).unwrap(),
            [PathBuf::from("helper.h"), PathBuf::from("tracked.c")].into()
        );
        fs::remove_file(dir.path().join("tracked.c")).unwrap();
        assert!(source_files(dir.path()).is_err());
    }
}
