//! Bind a prefix's Unix Steam bridge to the exact selected Wine's Unix ntdll.
//! The short loader-relative dependency fits existing Mach-O header padding.

#![forbid(unsafe_code)]

use anyhow::{Context, Result, bail};
use object::{Object, ObjectSymbol};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const DIRECT_DEPENDENCY: &str = "@loader_path/ntdll.so";

fn retarget_load_commands(bytes: &[u8]) -> Result<Vec<u8>> {
    let object::File::MachO64(macho) = object::File::parse(bytes)? else {
        bail!("Expected a 64-bit Mach-O bridge");
    };
    let mut replacements = Vec::new();
    let mut commands = macho.macho_load_commands()?;
    while let Some(command) = commands.next()? {
        if let Some(dylib) = command.dylib()? {
            let name = command.string(macho.endian(), dylib.dylib.name)?;
            if !is_ntdll_dependency(name) {
                continue;
            }
            let offset = command.raw_data().as_ptr() as usize - bytes.as_ptr() as usize;
            let name_offset = dylib.dylib.name.offset.get(macho.endian()) as usize;
            let end = offset + command.raw_data().len();
            let start = offset + name_offset;
            if DIRECT_DEPENDENCY.len() + 1 > end - start {
                bail!("Steam bridge load command has insufficient space for direct Wine binding");
            }
            replacements.push(start..end);
        }
    }
    if replacements.is_empty() {
        bail!("Steam bridge has no ntdll load command");
    }
    let mut updated = bytes.to_vec();
    for range in replacements {
        // Edit only the parsed command's name storage. No header growth, code
        // rewriting, library-ordinal changes, or external developer tools.
        updated[range.clone()].fill(0);
        updated[range.start..range.start + DIRECT_DEPENDENCY.len()]
            .copy_from_slice(DIRECT_DEPENDENCY.as_bytes());
    }
    Ok(updated)
}

fn is_ntdll_dependency(library: &[u8]) -> bool {
    let basename = library
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or(library);
    basename == b"ntdll_compat.so" || basename == b"ntdll.so"
}

/// Build a signed variant once per bridge/ntdll/ABI pair, independently of any
/// game prefix. The original bridge and the selected runtime remain untouched.
#[cfg(target_os = "macos")]
pub(crate) fn cached_library(
    source: &Path,
    runtime: &crate::runtime::ResolvedRuntime,
) -> Result<std::path::PathBuf> {
    let inputs = vec![source.to_path_buf(), runtime.unix_dir.join("ntdll.so")];
    let namespace = format!("steam-binding-v3-{:?}", runtime.mode);
    let identity = crate::staging::source_key(&namespace, &inputs)?;
    let root = crate::paths::support_dir()
        .join("cache/steam-bridge")
        .join(&identity);
    fs::create_dir_all(&root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".lock"))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;
    let output = root.join("lsteamclient.so");
    crate::staging::component(
        &root.join("receipt.json"),
        identity.clone(),
        &[],
        std::slice::from_ref(&output),
        || {
            let temp = tempfile::tempdir_in(&root)?;
            let steam = temp.path().join("drive_c/Program Files (x86)/Steam");
            fs::create_dir_all(&steam)?;
            fs::copy(source, steam.join("lsteamclient.so"))?;
            bind_prefix_for_runtime(temp.path(), runtime)?;
            anyhow::ensure!(
                crate::staging::source_key(&namespace, &inputs)? == identity,
                "Bridge input changed during binding"
            );
            crate::fs_util::atomic_write_file(&output, fs::read(steam.join("lsteamclient.so"))?)?;
            Ok(())
        },
    )?;
    Ok(output)
}

#[cfg(target_os = "macos")]
pub fn bind_prefix_for_runtime(
    prefix_dir: &Path,
    runtime: &crate::runtime::ResolvedRuntime,
) -> Result<()> {
    let steam_dir = prefix_dir.join("drive_c/Program Files (x86)/Steam");
    let libraries: Vec<_> = ["lsteamclient.so", "steamclient.so", "steamclient64.so"]
        .iter()
        .map(|name| steam_dir.join(name))
        .filter(|path| path.is_file())
        .collect();
    if libraries.is_empty() {
        return Ok(());
    }

    // Canonical paths make the symlink identify Wine's already-loaded image.
    let ntdll = runtime
        .unix_dir
        .join("ntdll.so")
        .canonicalize()
        .context("Selected Wine has no matching Unix ntdll for the Steam bridge")?;
    let ntdll_bytes = fs::read(&ntdll)?;
    let ntdll_object = object::File::parse(ntdll_bytes.as_slice())?;
    if ntdll_object.format() != object::BinaryFormat::MachO
        || ntdll_object.architecture() != runtime.arch()
    {
        bail!(
            "Selected Wine Unix ntdll has the wrong native ABI: {}",
            ntdll.display()
        );
    }
    let exported: BTreeSet<Vec<u8>> = ntdll_object
        .symbols()
        .filter(|symbol| symbol.is_definition() && symbol.is_global())
        .filter_map(|symbol| symbol.name_bytes().ok().map(Vec::from))
        .chain(
            ntdll_object
                .exports()?
                .iter()
                .map(|export| export.name().to_vec()),
        )
        .collect();

    // Validate the complete pairing before changing any load command or symlink.
    let mut plans = Vec::new();
    for library in &libraries {
        let bytes = fs::read(library)?;
        let object = object::File::parse(bytes.as_slice())?;
        if object.format() != object::BinaryFormat::MachO || object.architecture() != runtime.arch()
        {
            bail!(
                "Steam Unix bridge has the wrong native ABI: {}",
                library.display()
            );
        }
        if let object::File::MachO64(macho) = &object {
            let mut commands = macho.macho_load_commands()?;
            while let Some(command) = commands.next()? {
                if command.cmd() == object::macho::LC_CODE_SIGNATURE
                    && runtime.mode == crate::runtime::ExecutionMode::Intel
                {
                    bail!(
                        "Steam Unix bridge must be unsigned before relinking: {}",
                        library.display()
                    );
                }
            }
        }
        let mut dependencies = BTreeSet::new();
        let mut required = BTreeSet::new();
        for import in object.imports()? {
            if is_ntdll_dependency(import.library()) {
                dependencies.insert(String::from_utf8(import.library().to_vec())?);
                required.insert(import.name().to_vec());
            }
        }
        if dependencies.is_empty() || required.is_empty() {
            bail!(
                "Steam bridge has no declared ntdll dependency: {}",
                library.display()
            );
        }
        let missing: Vec<_> = required
            .difference(&exported)
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect();
        if !missing.is_empty() {
            bail!(
                "Selected Wine cannot directly bind {}: missing exports {}",
                library.display(),
                missing.join(", ")
            );
        }
        let bytes = if runtime.mode == crate::runtime::ExecutionMode::Intel {
            retarget_load_commands(&bytes)?
        } else {
            bytes
        };
        plans.push((library, bytes, dependencies));
    }

    // Prepare all replacements beside the prefix on the same filesystem. A failed
    // validation leaves the staged sources and current symlink untouched.
    let staging = tempfile::Builder::new()
        .prefix(".ntdll-binding-")
        .tempdir_in(&steam_dir)?;
    for (library, bytes, dependencies) in &plans {
        let temporary = staging.path().join(library.file_name().unwrap());
        fs::write(&temporary, bytes)?;
        fs::set_permissions(&temporary, fs::metadata(library)?.permissions())?;
        if runtime.mode == crate::runtime::ExecutionMode::Arm64Fex {
            // Only the staged private copy is modified. Strip its stale signature,
            // then sign the final load commands and verify before publishing it.
            let signed = object::File::parse(bytes.as_slice())?;
            if let object::File::MachO64(macho) = signed {
                let mut commands = macho.macho_load_commands()?;
                let mut has_signature = false;
                while let Some(command) = commands.next()? {
                    has_signature |= command.cmd() == object::macho::LC_CODE_SIGNATURE;
                }
                if has_signature {
                    signing_command(&["--remove-signature"], &temporary)?;
                }
            }
            for dependency in dependencies {
                let mut command = std::process::Command::new("/usr/bin/install_name_tool");
                command
                    .args(["-change", dependency, DIRECT_DEPENDENCY])
                    .arg(&temporary);
                crate::launch::clean_command(&mut command);
                let output = command
                    .output()
                    .context("Relink private ARM64 Steam bridge")?;
                anyhow::ensure!(
                    output.status.success(),
                    "Cannot relink private Steam bridge: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            signing_command(&["--force", "--sign", "-", "--timestamp=none"], &temporary)?;
            signing_command(&["--verify", "--strict"], &temporary)?;
        }
        let updated = fs::read(&temporary)?;
        let object = object::File::parse(updated.as_slice())?;
        if object.imports()?.iter().any(|import| {
            is_ntdll_dependency(import.library())
                && import.library() != DIRECT_DEPENDENCY.as_bytes()
        }) {
            bail!(
                "Steam bridge still refers to an old ntdll library: {}",
                library.display()
            );
        }
    }
    let temporary_link = staging.path().join("ntdll.so");
    std::os::unix::fs::symlink(&ntdll, &temporary_link)?;
    fs::rename(&temporary_link, steam_dir.join("ntdll.so"))?;
    for (library, _, _) in plans {
        fs::rename(staging.path().join(library.file_name().unwrap()), library)?;
    }
    let obsolete_adapter = steam_dir.join("ntdll_compat.so");
    if obsolete_adapter.is_file() || obsolete_adapter.is_symlink() {
        fs::remove_file(obsolete_adapter)?;
    }
    log::info!(
        "Steam bridge directly bound to selected Wine: {}",
        ntdll.display()
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn signing_command(arguments: &[&str], path: &Path) -> Result<()> {
    let mut command = std::process::Command::new("/usr/bin/codesign");
    command.args(arguments).arg(path);
    crate::launch::clean_command(&mut command);
    let output = command
        .output()
        .context("Run codesign on private Steam bridge")?;
    anyhow::ensure!(
        output.status.success(),
        "Steam bridge signing failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_arm64_bridge_binds_private_copy_and_missing_exports_leave_it_untouched() {
        let (temp, runtime) = crate::runtime::fixture(crate::runtime::ExecutionMode::Arm64Fex);
        let source = temp.path().join("ntdll.c");
        let ntdll = runtime.unix_dir.join("ntdll.so");
        fs::write(&source, "int wine_test(void) { return 42; }\n").unwrap();
        let compile = |source: &Path, output: &Path, dependency: Option<&Path>| {
            let mut c = std::process::Command::new("/usr/bin/clang");
            c.args(["-arch", "arm64", "-dynamiclib"])
                .arg(format!(
                    "-Wl,-install_name,@rpath/{}",
                    output.file_name().unwrap().to_string_lossy()
                ))
                .arg(source)
                .arg("-o")
                .arg(output);
            if let Some(dependency) = dependency {
                c.arg(dependency);
            }
            let out = c.output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        compile(&source, &ntdll, None);
        fs::write(
            &source,
            "extern int wine_test(void); int bridge_test(void) { return wine_test(); }\n",
        )
        .unwrap();
        let bridge = temp.path().join("source-bridge.so");
        compile(&source, &bridge, Some(&ntdll));
        signing_command(&["--force", "--sign", "-", "--timestamp=none"], &bridge).unwrap();
        let original = fs::read(&bridge).unwrap();
        let steam = temp.path().join("prefix/drive_c/Program Files (x86)/Steam");
        fs::create_dir_all(&steam).unwrap();
        let staged = steam.join("lsteamclient.so");
        fs::copy(&bridge, &staged).unwrap();
        bind_prefix_for_runtime(&temp.path().join("prefix"), &runtime).unwrap();
        signing_command(&["--verify", "--strict"], &staged).unwrap();
        assert_eq!(fs::read(&bridge).unwrap(), original);
        assert_eq!(fs::read_link(steam.join("ntdll.so")).unwrap(), ntdll);
        let bound = fs::read(&staged).unwrap();
        assert!(
            object::File::parse(bound.as_slice())
                .unwrap()
                .imports()
                .unwrap()
                .iter()
                .any(|i| i.library() == DIRECT_DEPENDENCY.as_bytes())
        );
        fs::write(&source, "int other_export(void) { return 0; }\n").unwrap();
        compile(&source, &ntdll, None);
        let error = bind_prefix_for_runtime(&temp.path().join("prefix"), &runtime).unwrap_err();
        assert!(error.to_string().contains("missing exports"));
        assert_eq!(fs::read(&staged).unwrap(), bound);
        assert_eq!(fs::read(&bridge).unwrap(), original);
    }
}
