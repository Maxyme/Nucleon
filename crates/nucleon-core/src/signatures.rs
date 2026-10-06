use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureDb {
    pub schema_version: u32,
    #[serde(default)]
    pub profile_version: u32,
    pub arch: String,
    pub platform: String,
    pub steam_build: u64,
    #[serde(default)]
    pub steam_build_date: String,
    pub signatures: Vec<SignatureEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureEntry {
    pub name: String,
    pub aob_hex: String,
    #[serde(default)]
    pub func_addr_this_build: Option<String>,
    #[serde(default)]
    pub match_offset: Option<i64>,
    #[serde(default)]
    pub anchor: Option<serde_json::Value>,
}

pub fn load_signature_db(path: &Path) -> Result<SignatureDb> {
    let data = fs::read_to_string(path)
        .with_context(|| format!("Failed to read signature file: {}", path.display()))?;
    let db: SignatureDb = serde_json::from_str(&data)
        .with_context(|| format!("Failed to parse signature JSON: {}", path.display()))?;
    Ok(db)
}

/// Detects the currently installed Steam client build number by inspecting
/// the package manifests in Steam.AppBundle.
pub fn detect_installed_steam_build() -> Option<u64> {
    let manifest_candidates = [
        crate::paths::home_dir().join("Library/Application Support/Steam/Steam.AppBundle/Steam/Contents/MacOS/package/steam_client_signed-2_osx.manifest"),
        crate::paths::home_dir().join("Library/Application Support/Steam/Steam.AppBundle/Steam/Contents/MacOS/package/steam_client_osx.manifest"),
        crate::paths::home_dir().join("Library/Application Support/Steam/package/steam_client_signed-2_osx.manifest"),
        crate::paths::home_dir().join("Library/Application Support/Steam/package/steam_client_osx.manifest"),
    ];

    for path in &manifest_candidates {
        if let Ok(content) = fs::read_to_string(path) {
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
        }
    }
    None
}

/// Finds the most suitable signature database for the installed or target Steam build.
/// If `target_build` is specified, it selects the database with the largest `db.steam_build <= target_build`.
/// If no database is <= target_build, it falls back to the earliest available database.
pub fn find_best_signature_db(
    dir: &Path,
    target_build: Option<u64>,
) -> Result<Option<(PathBuf, SignatureDb)>> {
    if !dir.is_dir() {
        return Ok(None);
    }

    let mut dbs: Vec<(PathBuf, SignatureDb)> = Vec::new();

    fn scan_dir(d: &Path, dbs: &mut Vec<(PathBuf, SignatureDb)>) {
        if let Ok(entries) = fs::read_dir(d) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    scan_dir(&path, dbs);
                } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Ok(db) = load_signature_db(&path) {
                        dbs.push((path, db));
                    }
                }
            }
        }
    }

    scan_dir(dir, &mut dbs);

    if dbs.is_empty() {
        return Ok(None);
    }

    // Sort by steam_build ascending
    dbs.sort_by_key(|(_, db)| db.steam_build);

    if let Some(target) = target_build {
        // Find the highest build <= target
        if let Some(matching) = dbs.iter().rfind(|(_, db)| db.steam_build <= target) {
            return Ok(Some(matching.clone()));
        }
        // If all available DBs are newer than target, return the earliest
        return Ok(dbs.first().cloned());
    }

    // Default: return the latest available DB
    Ok(dbs.last().cloned())
}

pub const DEFAULT_ARM64_TEMPLATE_JSON: &str = include_str!("../../../assets/template.json");

/// Extracts the ARM64 slice from a Mach-O binary (supporting Fat/Universal and single-arch binaries).
pub fn extract_arm64_slice(dylib_bytes: &[u8]) -> Result<&[u8]> {
    if dylib_bytes.len() < 8 {
        anyhow::bail!("File too small to be a valid Mach-O binary");
    }

    let magic = u32::from_be_bytes([
        dylib_bytes[0],
        dylib_bytes[1],
        dylib_bytes[2],
        dylib_bytes[3],
    ]);

    if magic == 0xcafebabe {
        let nfat_arch = u32::from_be_bytes([
            dylib_bytes[4],
            dylib_bytes[5],
            dylib_bytes[6],
            dylib_bytes[7],
        ]) as usize;

        for i in 0..nfat_arch {
            let offset_entry = 8 + i * 20;
            if dylib_bytes.len() < offset_entry + 20 {
                break;
            }
            let cputype = u32::from_be_bytes([
                dylib_bytes[offset_entry],
                dylib_bytes[offset_entry + 1],
                dylib_bytes[offset_entry + 2],
                dylib_bytes[offset_entry + 3],
            ]);
            let offset = u32::from_be_bytes([
                dylib_bytes[offset_entry + 8],
                dylib_bytes[offset_entry + 9],
                dylib_bytes[offset_entry + 10],
                dylib_bytes[offset_entry + 11],
            ]) as usize;
            let size = u32::from_be_bytes([
                dylib_bytes[offset_entry + 12],
                dylib_bytes[offset_entry + 13],
                dylib_bytes[offset_entry + 14],
                dylib_bytes[offset_entry + 15],
            ]) as usize;

            // CPU_TYPE_ARM64 = 0x0100000C
            if cputype == 0x0100000c && dylib_bytes.len() >= offset + size {
                return Ok(&dylib_bytes[offset..offset + size]);
            }
        }
        anyhow::bail!("arm64 slice not found in Mach-O universal binary");
    }

    // Single 64-bit Mach-O binary check (MH_MAGIC_64 = 0xfeedfacf)
    let magic_le = u32::from_le_bytes([
        dylib_bytes[0],
        dylib_bytes[1],
        dylib_bytes[2],
        dylib_bytes[3],
    ]);
    if magic_le == 0xfeedfacf {
        return Ok(dylib_bytes);
    }

    anyhow::bail!("Unsupported Mach-O magic: 0x{:08x}", magic);
}

/// Matches an Array of Bytes (AOB) with ?? wildcards against memory slice using memchr acceleration.
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

/// Scans the given ARM64 binary slice using a template database and generates a resolved SignatureDb.
pub fn scan_signatures_from_slice(
    arm64_slice: &[u8],
    template: &SignatureDb,
    steam_build: u64,
) -> SignatureDb {
    let mut resolved_db = template.clone();
    resolved_db.steam_build = steam_build;

    for sig in &mut resolved_db.signatures {
        if let Some(offset) = aob_scan_slice(arm64_slice, &sig.aob_hex) {
            let final_offset = (offset as i64 + sig.match_offset.unwrap_or(0)).max(0) as usize;
            sig.func_addr_this_build = Some(format!("0x{:x}", final_offset));
        }
    }

    resolved_db
}

/// Ensures that a signature profile exists locally for the installed Steam build.
///
/// 1. Checks local cache (~/Library/Application Support/nucleon/signatures/macos.arm64/<build>.json).
/// 2. If missing, scans steamclient.dylib once, generates the JSON profile, saves it locally, and returns it.
pub fn ensure_signature_db_for_installed_steam() -> Result<(PathBuf, SignatureDb)> {
    let build_id = detect_installed_steam_build().unwrap_or(0);
    let signatures_dir = crate::paths::signatures_dir();
    fs::create_dir_all(&signatures_dir)?;

    let cached_path = signatures_dir.join(format!("{build_id}.json"));
    if cached_path.is_file() {
        if let Ok(db) = load_signature_db(&cached_path) {
            return Ok((cached_path, db));
        }
    }

    // Cache miss: Locate steamclient.dylib on disk
    let steamclient_candidates = [
        crate::paths::steam_data_dir()
            .join("Steam.AppBundle/Steam/Contents/MacOS/steamclient.dylib"),
        crate::paths::steam_app().join("Contents/MacOS/steamclient.dylib"),
    ];

    let dylib_path = steamclient_candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("steamclient.dylib not found for signature scanning"))?;

    let dylib_bytes = fs::read(&dylib_path)
        .with_context(|| format!("Failed to read {}", dylib_path.display()))?;

    let arm64_slice = extract_arm64_slice(&dylib_bytes)?;

    // Parse template
    let template: SignatureDb = serde_json::from_str(DEFAULT_ARM64_TEMPLATE_JSON)
        .context("Failed to parse embedded ARM64 signature template")?;

    let resolved_db = scan_signatures_from_slice(arm64_slice, &template, build_id);

    // Save to local cache in Application Support (outside of git)
    let json_data = serde_json::to_string_pretty(&resolved_db)?;
    fs::write(&cached_path, json_data).with_context(|| {
        format!(
            "Failed to cache signature JSON at {}",
            cached_path.display()
        )
    })?;

    log::info!(
        "Successfully scanned and cached signature database for Steam build {} at {}",
        build_id,
        cached_path.display()
    );

    Ok((cached_path, resolved_db))
}

pub fn find_latest_signature_db(dir: &Path) -> Result<Option<(PathBuf, SignatureDb)>> {
    // 1. Check local cache or scan-once for currently installed Steam build
    if let Ok((path, db)) = ensure_signature_db_for_installed_steam() {
        return Ok(Some((path, db)));
    }

    // 2. Fallback to existing signature database directory
    let detected = detect_installed_steam_build();
    find_best_signature_db(dir, detected)
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
    fn test_embedded_template_parses() {
        let db: SignatureDb = serde_json::from_str(DEFAULT_ARM64_TEMPLATE_JSON).unwrap();
        assert_eq!(db.arch, "arm64");
        assert_eq!(db.platform, "macos");
        assert!(!db.signatures.is_empty());
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
    fn test_scan_signatures_from_slice() {
        let mut data = vec![0u8; 128];
        data[0x20] = 0xaa;
        data[0x21] = 0xbb;
        data[0x22] = 0xcc;
        data[0x23] = 0xdd;

        let template = SignatureDb {
            schema_version: 3,
            profile_version: 1,
            arch: "arm64".to_string(),
            platform: "macos".to_string(),
            steam_build: 0,
            steam_build_date: "".to_string(),
            signatures: vec![
                SignatureEntry {
                    name: "TestMatched".to_string(),
                    aob_hex: "AA BB CC DD".to_string(),
                    func_addr_this_build: None,
                    match_offset: Some(4),
                    anchor: None,
                },
                SignatureEntry {
                    name: "TestUnmatched".to_string(),
                    aob_hex: "EE FF 00 11".to_string(),
                    func_addr_this_build: None,
                    match_offset: None,
                    anchor: None,
                },
            ],
        };

        let resolved = scan_signatures_from_slice(&data, &template, 12345);
        assert_eq!(resolved.steam_build, 12345);

        let matched = resolved
            .signatures
            .iter()
            .find(|s| s.name == "TestMatched")
            .unwrap();
        assert_eq!(matched.func_addr_this_build.as_deref(), Some("0x24"));

        let unmatched = resolved
            .signatures
            .iter()
            .find(|s| s.name == "TestUnmatched")
            .unwrap();
        assert_eq!(unmatched.func_addr_this_build, None);
    }

    #[test]
    fn test_scan_real_steamclient_if_present() {
        let candidates = [
            crate::paths::steam_data_dir()
                .join("Steam.AppBundle/Steam/Contents/MacOS/steamclient.dylib"),
            crate::paths::steam_app().join("Contents/MacOS/steamclient.dylib"),
        ];
        if let Some(path) = candidates.iter().find(|p| p.is_file()) {
            let bytes = fs::read(path).expect("read steamclient.dylib");
            let slice = extract_arm64_slice(&bytes).expect("extract arm64 slice");
            let template: SignatureDb =
                serde_json::from_str(DEFAULT_ARM64_TEMPLATE_JSON).expect("parse template");
            let resolved = scan_signatures_from_slice(slice, &template, 1788652215);

            let init = resolved
                .signatures
                .iter()
                .find(|s| s.name == "CCompatManager::Init");
            assert!(init.is_some());
            assert!(init.unwrap().func_addr_this_build.is_some());
            assert_eq!(
                init.unwrap().func_addr_this_build.as_deref(),
                Some("0x690f6c")
            );

            let is_enabled = resolved
                .signatures
                .iter()
                .find(|s| s.name == "CCompatManager::BIsCompatibilityToolEnabled");
            assert!(is_enabled.is_some());
            assert_eq!(
                is_enabled.unwrap().func_addr_this_build.as_deref(),
                Some("0x6962f0")
            );
        }
    }

    #[test]
    fn test_ensure_signature_db_for_installed_steam() {
        if detect_installed_steam_build().is_some() {
            let res = ensure_signature_db_for_installed_steam();
            assert!(res.is_ok(), "ensure_signature_db failed: {:?}", res.err());
            let (path, db) = res.unwrap();
            assert!(path.is_file());
            assert!(db.steam_build > 0);
            assert!(!db.signatures.is_empty());
        }
    }
}
