use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
pub fn find_best_signature_db(dir: &Path, target_build: Option<u64>) -> Result<Option<(PathBuf, SignatureDb)>> {
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

pub fn find_latest_signature_db(dir: &Path) -> Result<Option<(PathBuf, SignatureDb)>> {
    let detected = detect_installed_steam_build();
    find_best_signature_db(dir, detected)
}

