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

/// Relink private prefix copies only. Sources in the bridge and Wine trees are untouched.
#[cfg(target_os = "macos")]
pub fn bind_prefix(prefix_dir: &Path, runner_dir: &Path) -> Result<()> {
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
    let ntdll = runner_dir
        .join("lib/wine/x86_64-unix/ntdll.so")
        .canonicalize()
        .context("Selected Wine has no x64 Unix ntdll for the Steam bridge")?;
    let ntdll_bytes = fs::read(&ntdll)?;
    let ntdll_object = object::File::parse(ntdll_bytes.as_slice())?;
    if ntdll_object.format() != object::BinaryFormat::MachO
        || ntdll_object.architecture() != object::Architecture::X86_64
    {
        bail!(
            "Selected Wine Unix ntdll must be x86_64 Mach-O: {}",
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
        if object.format() != object::BinaryFormat::MachO
            || object.architecture() != object::Architecture::X86_64
        {
            bail!(
                "Steam Unix bridge must be x86_64 Mach-O: {}",
                library.display()
            );
        }
        if let object::File::MachO64(macho) = &object {
            let mut commands = macho.macho_load_commands()?;
            while let Some(command) = commands.next()? {
                if command.cmd() == object::macho::LC_CODE_SIGNATURE {
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
        plans.push((library, retarget_load_commands(&bytes)?));
    }

    // Prepare all replacements beside the prefix on the same filesystem. A failed
    // validation leaves the staged sources and current symlink untouched.
    let staging = tempfile::Builder::new()
        .prefix(".ntdll-binding-")
        .tempdir_in(&steam_dir)?;
    for (library, bytes) in &plans {
        let temporary = staging.path().join(library.file_name().unwrap());
        fs::write(&temporary, bytes)?;
        fs::set_permissions(&temporary, fs::metadata(library)?.permissions())?;
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
    for (library, _) in plans {
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
