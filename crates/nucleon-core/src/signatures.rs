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

pub fn find_latest_signature_db(dir: &Path) -> Result<Option<(PathBuf, SignatureDb)>> {
    if !dir.is_dir() {
        return Ok(None);
    }

    let mut latest: Option<(u64, PathBuf, SignatureDb)> = None;

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Ok(db) = load_signature_db(&path) {
                let build = db.steam_build;
                if latest.as_ref().map(|(b, _, _)| build > *b).unwrap_or(true) {
                    latest = Some((build, path, db));
                }
            }
        }
    }

    Ok(latest.map(|(_, p, db)| (p, db)))
}
