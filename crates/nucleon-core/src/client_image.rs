#![forbid(unsafe_code)]

use crate::paths;
use anyhow::{Context, Result};
use std::fs;

/// Parses the Steam build number from the text content of a Steam package manifest.
/// Uses `keyvalues-parser` to navigate the VDF structure, falling back to line inspection.
pub fn parse_steam_manifest_build(content: &str) -> Option<u64> {
    if let Ok(partial) = keyvalues_parser::parse(content) {
        let vdf = keyvalues_parser::Vdf::from(partial);
        if let keyvalues_parser::Value::Obj(ref root_obj) = vdf.value
            && let Some(keyvalues_parser::Value::Str(version_str)) =
                root_obj.get("version").and_then(|v| v.first())
            && let Ok(v) = version_str.trim().parse::<u64>()
        {
            return Some(v);
        }
    }

    // Fallback: line-by-line inspection if non-standard or partial VDF
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("\"version\"") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                let v_str = parts[1].trim_matches('"');
                if let Ok(v) = v_str.parse::<u64>() {
                    return Some(v);
                }
            }
        }
    }

    None
}

/// Detects the currently installed Steam client build number by inspecting
/// the package manifests in Steam.AppBundle.
pub fn detect_installed_steam_build() -> Option<u64> {
    for path in paths::steam_client_manifest_candidates() {
        if let Ok(content) = fs::read_to_string(&path)
            && let Some(build) = parse_steam_manifest_build(&content)
        {
            return Some(build);
        }
    }
    None
}

/// Extracts the ARM64 slice from a Mach-O binary (supporting Fat/Universal and single-arch binaries).
pub fn extract_arm64_slice(dylib_bytes: &[u8]) -> Result<&[u8]> {
    crate::binary_image::slices(dylib_bytes, Some(object::macho::CPU_TYPE_ARM64))
        .context("Cannot extract arm64 Mach-O slice")?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("arm64 slice not found in Mach-O binary"))
}

/// Finds the first byte pattern match using memchr acceleration.
/// Whitespace separates hexadecimal bytes; `?` and `??` each match any one byte.
/// Empty, invalid, and longer-than-input patterns return `None`.
pub fn aob_scan_slice(data: &[u8], pattern: &str) -> Option<usize> {
    let tokens: Vec<&str> = pattern.split_whitespace().collect();
    if tokens.is_empty() || data.len() < tokens.len() {
        return None;
    }

    let mut bytes = Vec::with_capacity(tokens.len());
    let mut mask = Vec::with_capacity(tokens.len());

    for t in tokens {
        if t == "??" || t == "?" {
            bytes.push(0u8);
            mask.push(false);
        } else if let Ok(b) = u8::from_str_radix(t, 16) {
            bytes.push(b);
            mask.push(true);
        } else {
            return None;
        }
    }

    let pat_len = bytes.len();
    if data.len() < pat_len {
        return None;
    }

    // Find first non-wildcard anchor byte for fast skipping
    let first_idx = mask.iter().position(|&m| m).unwrap_or(0);
    let first_byte = bytes[first_idx];

    let mut i = 0;
    while i <= data.len() - pat_len {
        if mask[first_idx] && data[i + first_idx] != first_byte {
            if let Some(pos) = memchr::memchr(
                first_byte,
                &data[i + first_idx..data.len() - (pat_len - 1 - first_idx)],
            ) {
                i += pos;
            } else {
                break;
            }
        }

        let mut matched = true;
        for j in 0..pat_len {
            if mask[j] && data[i + j] != bytes[j] {
                matched = false;
                break;
            }
        }

        if matched {
            return Some(i);
        }

        i += 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aob_scan_slice_basic() {
        let data = vec![0x90, 0x90, 0xde, 0xad, 0xbe, 0xef, 0xcc];
        assert_eq!(aob_scan_slice(&data, "DE AD BE EF"), Some(2));
        assert_eq!(aob_scan_slice(&data, "DE ?? BE EF"), Some(2));
        assert_eq!(aob_scan_slice(&data, "BE EF ??"), Some(4));
        assert_eq!(aob_scan_slice(&data, "AA BB CC"), None);
    }

    #[test]
    fn wildcard_scanner_preserves_boundaries_and_pattern_syntax() {
        let cases: &[(&[u8], &str, Option<usize>)] = &[
            (&[], "", None),
            (&[], "?", None),
            (&[0xab], "  ", None),
            (&[0xab], "aB", Some(0)),
            (&[0xab], "?", Some(0)),
            (&[0xab], "??", Some(0)),
            (&[0xab], "AB ??", None),
            (&[0xab, 0xcd], "? ??", Some(0)),
            (&[0, 0xab, 0xcd], "AB CD", Some(1)),
            (&[0, 0xab, 0xcd], "? AB CD", Some(0)),
            (&[0, 0, 0xab], "?? AB", Some(1)),
            (&[0, 0xab, 0xcd], "AB ?", Some(1)),
            (&[0xab, 0xab, 0xcd], "AB CD", Some(1)),
            (&[0xab, 0xab, 0xcd], "AB AB", Some(0)),
            (&[0xab, 0xab, 0xcd], "AB CE", None),
            (&[0xab, 0xcd], " \tAB\ncd  ", Some(0)),
            (&[0xab], "GG", None),
            (&[0xab], "100", None),
            (&[0xab], "???", None),
            (&[0xab, 0xcd], "? GG", None),
        ];
        for &(data, pattern, expected) in cases {
            assert_eq!(
                aob_scan_slice(data, pattern),
                expected,
                "{data:?}: {pattern:?}"
            );
        }
    }

    #[test]
    fn test_extract_arm64_slice_single_arch() {
        let mut buf = vec![0u8; 32];
        buf[0..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
        buf[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
        let slice = extract_arm64_slice(&buf).expect("should extract single-arch slice");
        assert_eq!(slice.len(), 32);
    }

    #[test]
    fn test_extract_arm64_slice_fat_universal() {
        let mut buf = vec![0u8; 0x100];
        buf[0..4].copy_from_slice(&0xcafebabeu32.to_be_bytes());
        buf[4..8].copy_from_slice(&2u32.to_be_bytes());

        // Arch 0: x86_64
        buf[8..12].copy_from_slice(&0x01000007u32.to_be_bytes());
        buf[12..16].copy_from_slice(&3u32.to_be_bytes());
        buf[16..20].copy_from_slice(&0x40u32.to_be_bytes());
        buf[20..24].copy_from_slice(&0x20u32.to_be_bytes());
        buf[24..28].copy_from_slice(&12u32.to_be_bytes());

        // Arch 1: arm64
        buf[28..32].copy_from_slice(&0x0100000cu32.to_be_bytes());
        buf[32..36].copy_from_slice(&0u32.to_be_bytes());
        buf[36..40].copy_from_slice(&0x60u32.to_be_bytes());
        buf[40..44].copy_from_slice(&0x30u32.to_be_bytes());
        buf[44..48].copy_from_slice(&14u32.to_be_bytes());

        buf[0x60..0x90].fill(0x42);

        let slice = extract_arm64_slice(&buf).expect("should extract arm64 slice");
        assert_eq!(slice.len(), 0x30);
        assert_eq!(slice[0], 0x42);
    }

    #[test]
    fn test_extract_arm64_slice_fat64_universal() {
        let mut buf = vec![0u8; 0x100];
        buf[0..4].copy_from_slice(&0xcafebabfu32.to_be_bytes());
        buf[4..8].copy_from_slice(&1u32.to_be_bytes());

        // Arch 0: arm64 in 64-bit fat header (each entry is 32 bytes)
        buf[8..12].copy_from_slice(&0x0100000cu32.to_be_bytes()); // cputype
        buf[12..16].copy_from_slice(&0u32.to_be_bytes()); // cpusubtype
        buf[16..24].copy_from_slice(&0x50u64.to_be_bytes()); // offset (8 bytes)
        buf[24..32].copy_from_slice(&0x20u64.to_be_bytes()); // size (8 bytes)
        buf[32..36].copy_from_slice(&14u32.to_be_bytes()); // align (4 bytes)
        buf[36..40].copy_from_slice(&0u32.to_be_bytes()); // reserved (4 bytes)

        buf[0x50..0x70].fill(0x77);

        let slice = extract_arm64_slice(&buf).expect("should extract arm64 slice from fat64");
        assert_eq!(slice.len(), 0x20);
        assert_eq!(slice[0], 0x77);
    }

    #[test]
    fn test_extract_arm64_slice_fat_missing_arm64() {
        let mut buf = vec![0u8; 0x100];
        buf[0..4].copy_from_slice(&0xcafebabeu32.to_be_bytes());
        buf[4..8].copy_from_slice(&1u32.to_be_bytes());
        buf[8..12].copy_from_slice(&0x01000007u32.to_be_bytes());
        buf[12..16].copy_from_slice(&3u32.to_be_bytes());
        buf[16..20].copy_from_slice(&0x40u32.to_be_bytes());
        buf[20..24].copy_from_slice(&0x20u32.to_be_bytes());
        buf[24..28].copy_from_slice(&12u32.to_be_bytes());

        assert!(extract_arm64_slice(&buf).is_err());
    }

    #[test]
    fn test_parse_steam_manifest_build() {
        let sample_vdf = r#"
"osx"
{
	"version"		"1788652215"
	"ostype"		"macos1015"
	"tenfoot_images_all"
	{
		"file"		"tenfoot_images_all.zip.86419c7a56c12dd107b5e0d46f50c8a9b121f3cc"
		"size"		"6582204"
	}
}
"#;
        assert_eq!(parse_steam_manifest_build(sample_vdf), Some(1788652215));

        // Fallback test: malformed VDF with line match
        let malformed = "unclosed block {\n \"version\" \"1234567\"\n";
        assert_eq!(parse_steam_manifest_build(malformed), Some(1234567));

        // Missing version
        let no_version = "\"osx\" { \"other\" \"value\" }";
        assert_eq!(parse_steam_manifest_build(no_version), None);
    }
}
