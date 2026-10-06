use crate::paths;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const BRIDGE_MANIFEST_JSON: &str = include_str!("../../../assets/bridge-manifest.json");

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BridgeFileEntry {
    pub source: String,
    pub destination: String,
    #[serde(default)]
    pub required: bool,
}

/// Returns the expected Valve bridge files specification parsed from `bridge-manifest.json`.
pub fn bridge_manifest() -> Vec<BridgeFileEntry> {
    serde_json::from_str(BRIDGE_MANIFEST_JSON)
        .expect("Embedded bridge-manifest.json must be valid JSON")
}

/// Checks whether all required Valve bridge files are already staged.
pub fn is_bridge_staged(bridge_dir: &Path) -> bool {
    let manifest = bridge_manifest();
    manifest
        .iter()
        .filter(|e| e.required)
        .all(|e| bridge_dir.join(&e.destination).is_file())
}

/// Recursively or iteratively locates a source candidate file within `root`.
pub fn find_file_in_tree(root: &Path, rel_source: &str) -> Option<PathBuf> {
    let filename = Path::new(rel_source)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(rel_source);

    // 1. Exact relative candidate (e.g. "legacycompat/Steam.dll")
    let direct = root.join(rel_source);
    if direct.is_file() {
        return Some(direct);
    }
    // 2. Just the filename directly at root
    let direct_file = root.join(filename);
    if direct_file.is_file() {
        return Some(direct_file);
    }
    // 3. In root/legacycompat/<filename>
    let in_legacy = root.join("legacycompat").join(filename);
    if in_legacy.is_file() {
        return Some(in_legacy);
    }
    // 4. One-level deep subdirectories (e.g. ubuntu12/, win64/, *.extract/)
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let sub_candidate = path.join(rel_source);
                if sub_candidate.is_file() {
                    return Some(sub_candidate);
                }
                let sub_file = path.join(filename);
                if sub_file.is_file() {
                    return Some(sub_file);
                }
                let sub_legacy = path.join("legacycompat").join(filename);
                if sub_legacy.is_file() {
                    return Some(sub_legacy);
                }
            }
        }
    }
    None
}

/// Stages Valve bridge files into `bridge_dir` from an extracted directory using `bridge-manifest.json`.
/// Returns Ok(true) if all required files were found and copied.
pub fn stage_from_extracted_dir(source_dir: &Path, bridge_dir: &Path) -> Result<bool> {
    if !source_dir.is_dir() {
        return Ok(false);
    }

    let manifest = bridge_manifest();

    // Verify all required binaries exist before copying
    for entry in manifest.iter().filter(|e| e.required) {
        if find_file_in_tree(source_dir, &entry.source).is_none() {
            return Ok(false);
        }
    }

    // Copy all matching files to their final destinations
    for entry in &manifest {
        if let Some(src) = find_file_in_tree(source_dir, &entry.source) {
            let dest = bridge_dir.join(&entry.destination);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src, &dest).with_context(|| {
                format!("Failed to copy {} to {}", src.display(), dest.display())
            })?;
        }
    }

    Ok(true)
}

/// Ensures Valve bridge packages are staged, taking an optional custom extracted path
/// or reading the `NUCLEON_BRIDGE_PATH` environment variable.
pub fn stage_valve_bridge(custom_path: Option<&Path>) -> Result<()> {
    paths::ensure_dirs()?;
    let bridge_dir = paths::bridge_dir();

    let env_bridge = std::env::var_os("NUCLEON_BRIDGE_PATH").map(PathBuf::from);
    let candidate_source = custom_path.map(Path::to_path_buf).or(env_bridge);

    if let Some(ref source) = candidate_source {
        if !source.exists() {
            bail!(
                "Specified Valve bridge directory does not exist: {}",
                source.display()
            );
        }
        let staged = stage_from_extracted_dir(source, &bridge_dir)?;
        if !staged {
            bail!(
                "Valve bridge directory '{}' is missing required client binaries (expected: steamclient64.dll, tier0_s64.dll).",
                source.display()
            );
        }
        return Ok(());
    }

    if is_bridge_staged(&bridge_dir) {
        return Ok(());
    }

    bail!(
        "Valve client bridge libraries are not staged in {}.\n\
         To stage them, download and extract Valve's client packages, then run:\n\n  \
         nucleon setup --bridge-path <PATH_TO_EXTRACTED_DIR>\n\n\
         (or export NUCLEON_BRIDGE_PATH=<PATH_TO_EXTRACTED_DIR>)\n\n\
         See README.md for instructions.",
        bridge_dir.display()
    );
}

/// Backward-compatible alias for `stage_valve_bridge`.
pub fn fetch_and_stage_valve_packages(custom_path: Option<&Path>) -> Result<()> {
    stage_valve_bridge(custom_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_bridge_manifest_valid() {
        let manifest = bridge_manifest();
        assert!(!manifest.is_empty());
        let required_count = manifest.iter().filter(|e| e.required).count();
        assert!(
            required_count >= 2,
            "Must have at least 2 required bridge DLLs"
        );
    }

    #[test]
    fn test_is_bridge_staged() {
        let temp = tempdir().unwrap();
        let bridge_dir = temp.path().join("bridge");
        fs::create_dir_all(&bridge_dir).unwrap();

        assert!(!is_bridge_staged(&bridge_dir));

        // Create the required files
        fs::write(bridge_dir.join("steamclient64.dll"), "dummy").unwrap();
        fs::write(bridge_dir.join("tier0_s64.dll"), "dummy").unwrap();

        assert!(is_bridge_staged(&bridge_dir));
    }

    #[test]
    fn test_stage_from_extracted_dir() {
        let temp = tempdir().unwrap();
        let src_dir = temp.path().join("valve_extracted");
        let bridge_dir = temp.path().join("bridge");

        fs::create_dir_all(src_dir.join("legacycompat")).unwrap();
        fs::write(src_dir.join("steamclient64.dll"), "sc64").unwrap();
        fs::write(src_dir.join("tier0_s64.dll"), "tier0_64").unwrap();
        fs::write(src_dir.join("legacycompat/Steam.dll"), "steam_dll").unwrap();
        fs::write(src_dir.join("legacycompat/steamclient.dll"), "sc_dll").unwrap();

        let staged = stage_from_extracted_dir(&src_dir, &bridge_dir).unwrap();
        assert!(staged);

        assert_eq!(
            fs::read_to_string(bridge_dir.join("steamclient64.dll")).unwrap(),
            "sc64"
        );
        assert_eq!(
            fs::read_to_string(bridge_dir.join("tier0_s64.dll")).unwrap(),
            "tier0_64"
        );
        assert_eq!(
            fs::read_to_string(bridge_dir.join("legacycompat/Steam.dll")).unwrap(),
            "steam_dll"
        );
        assert_eq!(
            fs::read_to_string(bridge_dir.join("steamclient.dll")).unwrap(),
            "sc_dll"
        );
    }

    #[test]
    fn test_stage_from_extracted_dir_missing_required() {
        let temp = tempdir().unwrap();
        let empty_dir = temp.path().join("empty");
        let bridge_dir = temp.path().join("bridge");
        fs::create_dir_all(&empty_dir).unwrap();

        let staged = stage_from_extracted_dir(&empty_dir, &bridge_dir).unwrap();
        assert!(!staged);
    }
}
