//! File-level compatibility checks used before Automatic chooses a graphics path.
use anyhow::{Result, bail};
use object::{Architecture, BinaryFormat, Object, ObjectSection, ObjectSymbol};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub(crate) fn native_arch(path: &Path, arch: Architecture) -> bool {
    fs::read(path)
        .ok()
        .and_then(|data| {
            crate::binary_image::slices(&data, None).ok().map(|images| {
                images.iter().any(|image| {
                    object::File::parse(*image).is_ok_and(|f| {
                        f.format() == BinaryFormat::MachO && f.architecture() == arch
                    })
                })
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn wine_arch(root: &Path) -> Option<Architecture> {
    if root.join(crate::runtime::DESCRIPTOR).exists() {
        return crate::runtime::ResolvedRuntime::inspect(root)
            .ok()
            .map(|r| r.arch());
    }
    // Prefer the host ntdll over launch scripts or universal executable wrappers.
    for (dir, arch) in [
        ("x86_64-unix", Architecture::X86_64),
        ("aarch64-unix", Architecture::Aarch64),
    ] {
        for lib in ["lib/wine", "lib64/wine"] {
            if native_arch(&root.join(lib).join(dir).join("ntdll.so"), arch) {
                return Some(arch);
            }
        }
    }
    for name in ["bin/wine", "bin/wine64"] {
        let path = root.join(name);
        for arch in [Architecture::X86_64, Architecture::Aarch64] {
            if native_arch(&path, arch) {
                return Some(arch);
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) fn dxmt(bundle: &crate::dxmt::DxmtBundle, root: &Path, is64: bool) -> bool {
    let files = if is64 {
        [
            bundle.x64_d3d11.as_deref(),
            bundle.x64_dxgi.as_deref(),
            bundle.x64_winemetal.as_deref(),
        ]
    } else {
        [
            bundle.x86_d3d11.as_deref(),
            bundle.x86_dxgi.as_deref(),
            bundle.x86_winemetal.as_deref(),
        ]
    };
    files.into_iter().all(|p| pe(p, is64))
        && crate::dxmt::validate_runtime(bundle, Some(root)).is_ok()
        && wine_arch(root).is_some_and(|arch| {
            bundle
                .x64_winemetal_so
                .as_deref()
                .is_some_and(|p| native_arch(p, arch))
        })
}

pub(crate) fn pe(path: Option<&Path>, is64: bool) -> bool {
    path.and_then(|p| fs::read(p).ok()).is_some_and(|data| {
        pe_abi(&data).is_ok_and(|abi| abi == if is64 { PeAbi::X64 } else { PeAbi::X86 })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeAbi {
    X86,
    X64,
    Arm64,
    Arm64Ec,
    Arm64X,
}

/// Do not mistake AMD64 hybrid headers or ordinary ARM64 code for an x64 frontend.
pub(crate) fn pe_abi(data: &[u8]) -> Result<PeAbi> {
    use object::{endian::LittleEndian as LE, read::pe::ImageNtHeaders};
    let image = object::File::parse(data)?;
    if let object::File::Pe32(pe) = &image {
        anyhow::ensure!(
            pe.nt_headers().file_header().machine.get(LE) == object::pe::IMAGE_FILE_MACHINE_I386,
            "Unsupported 32-bit PE machine"
        );
        return Ok(PeAbi::X86);
    }
    let object::File::Pe64(pe) = &image else {
        bail!("Expected a Windows PE image");
    };
    let chpe =
        if let Some(directory) = pe.data_directory(object::pe::IMAGE_DIRECTORY_ENTRY_LOAD_CONFIG) {
            if directory.virtual_address.get(LE) == 0 {
                false
            } else {
                let config = directory.data(data, &pe.section_table())?;
                let size = config
                    .get(..4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
                    .unwrap_or(0);
                anyhow::ensure!(size <= config.len(), "Truncated PE load configuration");
                size >= 208
                    && config
                        .get(200..208)
                        .is_some_and(|b| b.iter().any(|v| *v != 0))
            }
        } else {
            false
        };
    Ok(match pe.nt_headers().file_header().machine.get(LE) {
        object::pe::IMAGE_FILE_MACHINE_AMD64 if chpe => PeAbi::Arm64Ec,
        object::pe::IMAGE_FILE_MACHINE_AMD64 => PeAbi::X64,
        object::pe::IMAGE_FILE_MACHINE_ARM64EC => PeAbi::Arm64Ec,
        object::pe::IMAGE_FILE_MACHINE_ARM64X => PeAbi::Arm64X,
        object::pe::IMAGE_FILE_MACHINE_ARM64 if chpe => PeAbi::Arm64X,
        object::pe::IMAGE_FILE_MACHINE_ARM64 => PeAbi::Arm64,
        _ => bail!("Unsupported 64-bit PE machine"),
    })
}

#[cfg(test)]
pub(crate) fn vulkan(icd: &Path, root: &Path) -> bool {
    let Some(arch) = wine_arch(root) else {
        return false;
    };
    vulkan_arch(icd, arch)
}

pub(crate) fn vulkan_arch(icd: &Path, arch: Architecture) -> bool {
    let Ok(manifest) = crate::configuration::resolve_vulkan_icd(icd) else {
        return false;
    };
    if !manifest
        .api_version()
        .is_some_and(|version| version >= (1, 3, 0))
    {
        return false;
    }
    native_arch(&manifest.library, arch)
}

pub(crate) fn has_export(data: &[u8], aliases: &[&str]) -> bool {
    crate::binary_image::slices(data, None).is_ok_and(|images| {
        images.iter().any(|image| {
            object::File::parse(*image).is_ok_and(|f| {
                f.symbols().chain(f.dynamic_symbols()).any(|s| {
                    s.is_definition()
                        && s.is_global()
                        && s.name()
                            .is_ok_and(|name| aliases.contains(&name.trim_start_matches('_')))
                }) || f.exports().is_ok_and(|exports| {
                    exports.iter().any(|e| {
                        std::str::from_utf8(e.name())
                            .is_ok_and(|name| aliases.contains(&name.trim_start_matches('_')))
                    })
                })
            })
        })
    })
}

/// Compare loadable code/data, architecture, and UUID, excluding code-signature blobs.
pub(crate) fn hook_identity(data: &[u8]) -> Result<Vec<[u8; 32]>> {
    let mut hashes = Vec::new();
    for image in crate::binary_image::slices(data, None)? {
        let f = object::File::parse(image)?;
        if f.format() != BinaryFormat::MachO || f.kind() != object::ObjectKind::Dynamic {
            bail!("Hook must be a Mach-O dynamic library");
        }
        let mut hash = Sha256::new();
        hash.update(format!("{:?}", f.architecture()));
        if let Some(uuid) = f.mach_uuid()? {
            hash.update(uuid);
        }
        let mut count = 0;
        for section in f.sections() {
            hash.update(section.segment_name()?.unwrap_or("").as_bytes());
            hash.update(section.name()?.as_bytes());
            hash.update(section.address().to_le_bytes());
            hash.update(section.size().to_le_bytes());
            hash.update(section.data()?);
            count += 1;
        }
        if count == 0 {
            bail!("Hook has no loadable sections");
        }
        hashes.push(hash.finalize().into());
    }
    if hashes.is_empty() {
        bail!("Hook contains no architectures");
    }
    hashes.sort();
    Ok(hashes)
}

#[cfg(test)]
pub(crate) fn test_image(symbol: &str, text: u8) -> Vec<u8> {
    // A small valid x86_64 dylib with one text section and an exported symbol.
    let mut data = vec![0u8; 256];
    let mut put =
        |offset: usize, value: u32| data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    for (offset, value) in [
        (0, 0xfeedfacf),
        (4, 0x01000007),
        (8, 3),
        (12, 6),
        (16, 2),
        (20, 176),
        (32, 0x19),
        (36, 152),
        (96, 1),
        (184, 2),
        (188, 24),
        (192, 224),
        (196, 1),
        (200, 240),
        (204, (symbol.len() + 3) as u32),
        (152, 208),
        (224, 1),
    ] {
        put(offset, value);
    }
    data[40..46].copy_from_slice(b"__TEXT");
    data[104..110].copy_from_slice(b"__text");
    data[120..126].copy_from_slice(b"__TEXT");
    data[136..144].copy_from_slice(&208u64.to_le_bytes());
    data[144..152].copy_from_slice(&8u64.to_le_bytes());
    data[208..216].fill(text);
    data[228] = 0x0f;
    data[229] = 1;
    data[232..240].copy_from_slice(&208u64.to_le_bytes());
    data.truncate(241);
    data.push(b'_');
    data.extend_from_slice(symbol.as_bytes());
    data.push(0);
    data
}

#[cfg(test)]
pub(crate) fn test_pe_image(machine: u16) -> Vec<u8> {
    let is64 = machine != 0x14c;
    let mut b = vec![0u8; 512];
    b[..2].copy_from_slice(b"MZ");
    b[60..64].copy_from_slice(&128u32.to_le_bytes());
    b[128..132].copy_from_slice(b"PE\0\0");
    b[132..134].copy_from_slice(&machine.to_le_bytes());
    b[148..150].copy_from_slice(&(if is64 { 240u16 } else { 224u16 }).to_le_bytes());
    b[152..154].copy_from_slice(&(if is64 { 0x20bu16 } else { 0x10bu16 }).to_le_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_metadata_is_not_a_plain_amd64_frontend() {
        let mut data = test_pe_image(0x8664);
        data.resize(768, 0);
        data[134..136].copy_from_slice(&1u16.to_le_bytes());
        data[260..264].copy_from_slice(&16u32.to_le_bytes());
        data[344..348].copy_from_slice(&0x1000u32.to_le_bytes());
        data[348..352].copy_from_slice(&208u32.to_le_bytes());
        data[400..404].copy_from_slice(&256u32.to_le_bytes());
        data[404..408].copy_from_slice(&0x1000u32.to_le_bytes());
        data[408..412].copy_from_slice(&256u32.to_le_bytes());
        data[412..416].copy_from_slice(&512u32.to_le_bytes());
        data[512..516].copy_from_slice(&208u32.to_le_bytes());
        assert_eq!(pe_abi(&data).unwrap(), PeAbi::X64);
        data[712..720].copy_from_slice(&0x140001100u64.to_le_bytes());
        assert_eq!(pe_abi(&data).unwrap(), PeAbi::Arm64Ec);
        data[132..134].copy_from_slice(&0xaa64u16.to_le_bytes());
        assert_eq!(pe_abi(&data).unwrap(), PeAbi::Arm64X);
        assert_eq!(pe_abi(&test_pe_image(0xaa64)).unwrap(), PeAbi::Arm64);
        data[512..516].copy_from_slice(&209u32.to_le_bytes());
        assert!(pe_abi(&data).is_err());
    }

    fn pe_image(is64: bool) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[..2].copy_from_slice(b"MZ");
        b[60..64].copy_from_slice(&128u32.to_le_bytes());
        b[128..132].copy_from_slice(b"PE\0\0");
        b[132..134].copy_from_slice(&(if is64 { 0x8664u16 } else { 0x14cu16 }).to_le_bytes());
        b[148..150].copy_from_slice(&(if is64 { 240u16 } else { 224u16 }).to_le_bytes());
        b[152..154].copy_from_slice(&(if is64 { 0x20bu16 } else { 0x10bu16 }).to_le_bytes());
        b
    }

    #[test]
    fn pe_preflight_rejects_wrong_architecture_and_placeholder_files() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join("dxgi.dll");
        fs::write(&dll, pe_image(true)).unwrap();
        assert!(pe(Some(&dll), true));
        assert!(!pe(Some(&dll), false));
        fs::write(&dll, b"placeholder").unwrap();
        assert!(!pe(Some(&dll), true));
        assert!(!pe(None, true));
    }

    #[test]
    fn vulkan_preflight_checks_relative_library_host_arch_and_api_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wine");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/wine"), test_image("wine", 1)).unwrap();
        let icd = dir.path().join("driver.json");
        let library = dir.path().join("driver.dylib");
        let write_icd = |version| {
            fs::write(
                &icd,
                serde_json::json!({"ICD":{"library_path":"driver.dylib","api_version":version}})
                    .to_string(),
            )
            .unwrap()
        };
        write_icd("1.4.0");
        assert!(!vulkan(&icd, &root));
        let mut arm = test_image("driver", 1);
        arm[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
        fs::write(&library, arm).unwrap();
        assert!(!vulkan(&icd, &root));
        fs::write(&library, test_image("driver", 1)).unwrap();
        assert!(vulkan(&icd, &root));
        for version in ["1.3.0", "1.4.0", "2.0.0"] {
            write_icd(version);
            assert!(vulkan(&icd, &root), "{version}");
        }
        for version in [
            "1.2.999",
            "",
            "garbage.1.3",
            "1.garbage.3",
            "1.3.bad",
            "1.3",
            "1.3.0.0",
            "+1.3.0",
            "4294967296.3.0",
        ] {
            write_icd(version);
            assert!(!vulkan(&icd, &root), "{version}");
        }
        fs::write(&icd, r#"{"ICD":{"library_path":"driver.dylib"}}"#).unwrap();
        // Readable driver configuration alone does not establish Vulkan 1.3.
        crate::configuration::vulkan_manifest("test", &icd).unwrap();
        assert!(!vulkan(&icd, &root));
    }

    #[test]
    fn dxmt_preflight_rejects_incomplete_and_mismatched_builtin_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wine");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/wine"), test_image("wine", 1)).unwrap();
        let bundle_dir = dir.path().join("dxmt");
        fs::create_dir_all(&bundle_dir).unwrap();
        fs::write(bundle_dir.join("d3d11.dll"), pe_image(true)).unwrap();
        let bundle = crate::dxmt::inspect_candidate_dir(&bundle_dir).unwrap();
        assert!(!dxmt(&bundle, &root, true));
        for name in ["dxgi.dll", "winemetal.dll"] {
            fs::write(bundle_dir.join(name), pe_image(true)).unwrap();
        }
        fs::write(bundle_dir.join("winemetal.so"), test_image("metal", 1)).unwrap();
        let bundle = crate::dxmt::inspect_candidate_dir(&bundle_dir).unwrap();
        assert!(dxmt(&bundle, &root, true));
        assert!(!dxmt(&bundle, &root, false));
        let mut builtin = pe_image(true);
        builtin[64..80].copy_from_slice(b"Wine builtin DLL");
        fs::write(bundle_dir.join("d3d11.dll"), builtin).unwrap();
        assert!(!dxmt(&bundle, &root, true));
    }

    #[test]
    fn capabilities_require_exports_rather_than_embedded_strings() {
        assert!(!has_export(
            b"D3DKMTEnumAdapters2",
            &["D3DKMTEnumAdapters2"]
        ));
        let mut data = test_image("unrelated", 1);
        data.extend_from_slice(b"D3DKMTEnumAdapters2");
        assert!(!has_export(&data, &["D3DKMTEnumAdapters2"]));
        assert!(has_export(
            &test_image("D3DKMTEnumAdapters2", 1),
            &["D3DKMTEnumAdapters2"]
        ));
    }
}
