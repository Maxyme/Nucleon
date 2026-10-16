//! Shared universal-image splitting. Consumers validate the selected image's contents.
use anyhow::Result;
use object::{
    Endianness, FileKind, macho,
    read::macho::{FatArch, MachHeader, MachOFatFile32, MachOFatFile64},
};

/// Split a universal image or return the original thin image. An optional Mach-O CPU
/// filter selects the first matching entry before checking its range: a corrupt unrelated
/// architecture must not prevent extracting an otherwise valid slice. Without a filter,
/// every range is checked so runtime audits cannot silently skip corrupt entries.
pub(crate) fn slices(data: &[u8], cpu: Option<u32>) -> Result<Vec<&[u8]>> {
    Ok(match FileKind::parse(data)? {
        FileKind::MachOFat32 => fat_slices(MachOFatFile32::parse(data)?.arches(), data, cpu)?,
        FileKind::MachOFat64 => fat_slices(MachOFatFile64::parse(data)?.arches(), data, cpu)?,
        _ => {
            if let Some(cpu) = cpu {
                let header = macho::MachHeader64::<Endianness>::parse(data, 0)?;
                if header.cputype(header.endian()?) != cpu {
                    return Ok(Vec::new());
                }
            }
            vec![data]
        }
    })
}

fn fat_slices<'a, Fat: FatArch>(
    arches: &[Fat],
    data: &'a [u8],
    cpu: Option<u32>,
) -> Result<Vec<&'a [u8]>> {
    arches
        .iter()
        .filter(|arch| cpu.is_none_or(|cpu| arch.cputype() == cpu))
        .take(if cpu.is_some() { 1 } else { usize::MAX })
        .map(|arch| Ok(arch.data(data)?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thin(cpu: u32, big_endian: bool) -> Vec<u8> {
        let mut image = vec![0; 32];
        for (offset, value) in [(0, macho::MH_MAGIC_64), (4, cpu)] {
            image[offset..offset + 4].copy_from_slice(&if big_endian {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            });
        }
        image
    }

    fn universal(fat64: bool) -> Vec<u8> {
        let mut image = vec![0; 256];
        let magic = if fat64 {
            macho::FAT_MAGIC_64
        } else {
            macho::FAT_MAGIC
        };
        image[..4].copy_from_slice(&magic.to_be_bytes());
        image[4..8].copy_from_slice(&2u32.to_be_bytes());
        let entry_size = if fat64 { 32 } else { 20 };
        for (index, cpu) in [macho::CPU_TYPE_X86_64, macho::CPU_TYPE_ARM64]
            .into_iter()
            .enumerate()
        {
            let header = 8 + index * entry_size;
            let offset = 128 + index * 32;
            image[header..header + 4].copy_from_slice(&cpu.to_be_bytes());
            if fat64 {
                image[header + 8..header + 16].copy_from_slice(&(offset as u64).to_be_bytes());
                image[header + 16..header + 24].copy_from_slice(&32u64.to_be_bytes());
            } else {
                image[header + 8..header + 12].copy_from_slice(&(offset as u32).to_be_bytes());
                image[header + 12..header + 16].copy_from_slice(&32u32.to_be_bytes());
            }
            image[offset..offset + 32].copy_from_slice(&thin(cpu, false));
        }
        image
    }

    #[test]
    fn thin_cpu_selection_uses_header_endianness() {
        for big_endian in [false, true] {
            for cpu in [macho::CPU_TYPE_ARM64, macho::CPU_TYPE_X86_64] {
                let image = thin(cpu, big_endian);
                assert_eq!(slices(&image, None).unwrap(), vec![image.as_slice()]);
                let selected = slices(&image, Some(macho::CPU_TYPE_ARM64)).unwrap();
                assert_eq!(selected.len(), usize::from(cpu == macho::CPU_TYPE_ARM64));
                if let Some(slice) = selected.first() {
                    assert_eq!(slice.as_ptr(), image.as_ptr());
                }
            }
        }
    }

    #[test]
    fn universal_formats_keep_order_and_borrow_selected_bytes() {
        for fat64 in [false, true] {
            let image = universal(fat64);
            assert_eq!(
                slices(&image, None).unwrap(),
                vec![&image[128..160], &image[160..192]]
            );
            let arm = slices(&image, Some(macho::CPU_TYPE_ARM64)).unwrap();
            assert_eq!(arm, vec![&image[160..192]]);
            assert_eq!(arm[0].as_ptr(), image[160..].as_ptr());
            assert!(
                slices(&image, Some(macho::CPU_TYPE_POWERPC))
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn truncated_tables_and_invalid_ranges_fail_without_falling_back_to_thin() {
        for fat64 in [false, true] {
            let mut image = universal(fat64);
            assert!(slices(&image[..12], None).is_err());
            // An invalid x86 range cannot block extracting a valid ARM slice.
            if fat64 {
                image[16..24].copy_from_slice(&u64::MAX.to_be_bytes());
            } else {
                image[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
            }
            assert!(slices(&image, None).is_err());
            assert!(slices(&image, Some(macho::CPU_TYPE_X86_64)).is_err());
            assert_eq!(
                slices(&image, Some(macho::CPU_TYPE_ARM64)).unwrap(),
                vec![&image[160..192]]
            );
        }
        for fat64 in [false, true] {
            let mut image = universal(fat64);
            let header = if fat64 { 40 } else { 28 };
            if fat64 {
                image[header + 8..header + 16].copy_from_slice(&u64::MAX.to_be_bytes());
            } else {
                image[header + 8..header + 12].copy_from_slice(&u32::MAX.to_be_bytes());
            }
            assert!(crate::client_image::extract_arm64_slice(&image).is_err());
        }
        assert!(slices(&[], None).is_err());
        assert!(slices(&[0; 32], Some(macho::CPU_TYPE_ARM64)).is_err());
        assert!(
            slices(
                &thin(macho::CPU_TYPE_ARM64, false)[..16],
                Some(macho::CPU_TYPE_ARM64)
            )
            .is_err()
        );
    }
}
