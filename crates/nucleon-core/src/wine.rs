use crate::paths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomWineRecord {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineRuntime {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub tool_id: String,
    pub root: PathBuf,
    pub version: Option<String>,
}

fn query_wine_version(wine_bin: &Path) -> Option<String> {
    let output = Command::new(wine_bin).arg("--version").output().ok()?;
    if output.status.success() {
        let v = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    None
}

/// Inspects a directory to check if it contains a valid Wine runtime.
/// Handles standard Wine prefixes, app bundles (Contents/Resources/wine), and custom directories.
pub fn inspect_wine_dir(
    dir: &Path,
    id_override: Option<&str>,
    name_override: Option<&str>,
) -> Option<WineRuntime> {
    if !dir.is_dir() {
        return None;
    }

    // Resolve actual runtime root (checking if dir itself or Contents/Resources/wine contains bin/wine)
    let actual_root = if dir.join("bin/wine").is_file() && dir.join("bin/wineserver").is_file() {
        dir.to_path_buf()
    } else if dir.join("Contents/Resources/wine/bin/wine").is_file() {
        dir.join("Contents/Resources/wine")
    } else {
        return None;
    };

    let wine_bin = actual_root.join("bin/wine");
    let version = query_wine_version(&wine_bin);

    // Derive names and IDs from directory name or overrides
    let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("wine");
    let lower = dir_name.to_lowercase();

    let (id, name, display_name, tool_id) =
        if let (Some(id), Some(name)) = (id_override, name_override) {
            let tool = format!("nucleon-wine-{}", id.trim_start_matches("nucleon-wine-"));
            let display = format!("Nucleon (Wine: {name})");
            (id.to_string(), name.to_string(), display, tool)
        } else if lower.contains("crossover") {
            (
                "crossover".to_string(),
                "Heroic Wine-CrossOver".to_string(),
                "Nucleon (Wine: CrossOver)".to_string(),
                "nucleon-wine-crossover".to_string(),
            )
        } else if lower.contains("dxmt") {
            (
                "staging-dxmt".to_string(),
                "Heroic Wine-Staging (DXMT)".to_string(),
                "Nucleon (Wine: DXMT)".to_string(),
                "nucleon-wine-dxmt".to_string(),
            )
        } else if lower.contains("staging") {
            let is_heroic = dir.to_string_lossy().to_lowercase().contains("heroic");
            let name = if is_heroic {
                "Heroic Wine-Staging"
            } else {
                "Wine-Staging"
            };
            (
                "staging".to_string(),
                name.to_string(),
                "Nucleon (Wine: Staging)".to_string(),
                "nucleon-wine-staging".to_string(),
            )
        } else {
            let slug = dir_name.to_lowercase().replace([' ', '_'], "-");
            (
                slug.clone(),
                dir_name.to_string(),
                format!("Nucleon (Wine: {dir_name})"),
                format!("nucleon-wine-{slug}"),
            )
        };

    Some(WineRuntime {
        id,
        name,
        display_name,
        tool_id,
        root: actual_root,
        version,
    })
}

pub fn custom_wine_path_file() -> PathBuf {
    paths::support_dir().join("custom_wine_path.txt")
}

pub fn custom_wines_file() -> PathBuf {
    paths::support_dir().join("custom_wines.json")
}

pub fn load_custom_wines() -> Vec<CustomWineRecord> {
    let file = custom_wines_file();
    if !file.is_file() {
        return Vec::new();
    }
    match fs::read_to_string(&file) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn save_custom_wines(records: &[CustomWineRecord]) -> Result<()> {
    paths::ensure_dirs()?;
    let json = serde_json::to_string_pretty(records)?;
    fs::write(custom_wines_file(), json)?;
    Ok(())
}

/// Discovers all available Wine runtimes on the system, including Heroic, Homebrew,
/// CrossOver, Whisky, system installations, and custom paths.
pub fn discover_wine_runtimes() -> Vec<WineRuntime> {
    let mut runtimes = Vec::new();
    let mut seen_roots = std::collections::HashSet::new();

    // 1. Explicit environment variable: NUCLEON_WINE_PATH or WINE_PATH
    for var in &["NUCLEON_WINE_PATH", "WINE_PATH"] {
        if let Ok(p) = std::env::var(var) {
            let path = PathBuf::from(p);
            if let Some(rt) = inspect_wine_dir(&path, Some("custom"), Some("Custom Wine")) {
                if seen_roots.insert(rt.root.clone()) {
                    runtimes.push(rt);
                }
            }
        }
    }

    // 2. Custom runtimes persisted via `custom_wines.json`
    for record in load_custom_wines() {
        if let Some(rt) = inspect_wine_dir(&record.path, Some(&record.id), Some(&record.name)) {
            if seen_roots.insert(rt.root.clone()) {
                runtimes.push(rt);
            }
        }
    }

    // 2b. Legacy custom path persisted via `custom_wine_path.txt`
    let custom_file = custom_wine_path_file();
    if custom_file.is_file() {
        if let Ok(content) = fs::read_to_string(&custom_file) {
            let p = PathBuf::from(content.trim());
            if let Some(rt) = inspect_wine_dir(&p, Some("custom"), Some("Custom Wine")) {
                if seen_roots.insert(rt.root.clone()) {
                    runtimes.push(rt);
                }
            }
        }
    }

    // 3. Scan Heroic Games Launcher tools directory
    let heroic_wine_dir = paths::home_dir().join("Library/Application Support/heroic/tools/wine");
    if heroic_wine_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&heroic_wine_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    if let Some(rt) = inspect_wine_dir(&p, None, None) {
                        if seen_roots.insert(rt.root.clone()) {
                            runtimes.push(rt);
                        }
                    }
                }
            }
        }
    }

    // 4. Scan Homebrew Wine installations
    let brew_candidates = [
        PathBuf::from("/opt/homebrew/opt/wine-staging/Contents/Resources/wine"),
        PathBuf::from("/opt/homebrew/opt/wine-staging"),
        PathBuf::from("/usr/local/opt/wine-staging/Contents/Resources/wine"),
        PathBuf::from("/usr/local/opt/wine-staging"),
    ];
    for cand in &brew_candidates {
        if let Some(rt) =
            inspect_wine_dir(cand, Some("brew-staging"), Some("Homebrew Wine-Staging"))
        {
            if seen_roots.insert(rt.root.clone()) {
                runtimes.push(rt);
            }
        }
    }

    // 5. Scan Whisky and CrossOver installations
    let app_candidates = [
        (
            paths::home_dir().join("Library/Application Support/Whisky/Libraries/Wine"),
            "whisky",
            "Whisky Wine",
        ),
        (
            PathBuf::from("/Applications/CrossOver.app/Contents/SharedSupport/CrossOver"),
            "crossover-app",
            "CrossOver Wine",
        ),
        (
            PathBuf::from("/Applications/Wine Staging.app/Contents/Resources/wine"),
            "staging-app",
            "Wine Staging App",
        ),
    ];
    for (cand, id, name) in &app_candidates {
        if let Some(rt) = inspect_wine_dir(cand, Some(id), Some(name)) {
            if seen_roots.insert(rt.root.clone()) {
                runtimes.push(rt);
            }
        }
    }

    // 6. Check all Nucleon runners in runners directory
    let runners_dir = paths::runners_dir();
    if let Ok(entries) = fs::read_dir(&runners_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let id_override = if p.file_name().and_then(|n| n.to_str()) == Some("wine-staging")
                {
                    Some("staged")
                } else {
                    None
                };
                if let Some(rt) = inspect_wine_dir(&p, id_override, None) {
                    if seen_roots.insert(rt.root.clone()) {
                        runtimes.push(rt);
                    }
                }
            }
        }
    }

    // 7. Check Game Porting Toolkit installations
    let gptk_candidates = [
        (
            PathBuf::from("/Applications/Game Porting Toolkit.app/Contents/Resources/wine"),
            "gptk-app",
            "Game Porting Toolkit App",
        ),
        (
            PathBuf::from("/Applications/Game Porting Toolkit.app"),
            "gptk-app",
            "Game Porting Toolkit App",
        ),
        (
            PathBuf::from("/opt/homebrew/opt/game-porting-toolkit/Contents/Resources/wine"),
            "brew-gptk",
            "Homebrew Game Porting Toolkit",
        ),
        (
            PathBuf::from("/opt/homebrew/opt/game-porting-toolkit"),
            "brew-gptk",
            "Homebrew Game Porting Toolkit",
        ),
    ];
    for (cand, id, name) in &gptk_candidates {
        if let Some(rt) = inspect_wine_dir(cand, Some(id), Some(name)) {
            if seen_roots.insert(rt.root.clone()) {
                runtimes.push(rt);
            }
        }
    }

    let custom_ids: std::collections::HashSet<String> =
        load_custom_wines().into_iter().map(|r| r.id).collect();

    runtimes.sort_by_key(|r| {
        if r.id == "custom" || custom_ids.contains(&r.id) {
            0
        } else {
            match r.id.as_str() {
                "staging" => 1,
                "brew-staging" => 2,
                "crossover" => 3,
                "staging-dxmt" => 4,
                "staged" => 5,
                "brew-gptk" | "gptk-app" => 6,
                _ => 7,
            }
        }
    });

    runtimes
}

/// Returns the primary/recommended Wine runtime (preferring Wine-Staging or Heroic Wine).
pub fn find_primary_wine_runtime() -> Option<WineRuntime> {
    let runtimes = discover_wine_runtimes();
    if runtimes.is_empty() {
        return None;
    }

    // Prefer custom runtime if configured
    let custom_ids: std::collections::HashSet<String> =
        load_custom_wines().into_iter().map(|r| r.id).collect();
    if let Some(custom) = runtimes
        .iter()
        .find(|r| r.id == "custom" || custom_ids.contains(&r.id))
    {
        return Some(custom.clone());
    }

    // Prefer staging if available, else first detected
    if let Some(staging) = runtimes
        .iter()
        .find(|r| r.id == "staging" || r.id == "brew-staging")
    {
        return Some(staging.clone());
    }

    Some(runtimes[0].clone())
}

/// Registers a custom named Wine runtime and validates it.
pub fn add_custom_wine(name_or_id: &str, path: &Path) -> Result<WineRuntime> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let trimmed = name_or_id.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Custom Wine runtime name cannot be empty");
    }

    let slug = trimmed.to_lowercase().replace([' ', '_'], "-");

    let runtime = inspect_wine_dir(&canonical, Some(&slug), Some(trimmed))
        .context("Specified path does not contain a valid Wine runtime (bin/wine not found)")?;

    let mut records = load_custom_wines();
    if let Some(existing) = records.iter_mut().find(|r| r.id == slug) {
        existing.name = trimmed.to_string();
        existing.path = canonical.clone();
    } else {
        records.push(CustomWineRecord {
            id: slug,
            name: trimmed.to_string(),
            path: canonical.clone(),
        });
    }

    save_custom_wines(&records)?;
    log::info!(
        "Registered custom Wine runtime '{}' at {}",
        trimmed,
        runtime.root.display()
    );
    Ok(runtime)
}

/// Unregisters a custom named Wine runtime by name or ID.
pub fn remove_custom_wine(name_or_id: &str) -> Result<bool> {
    let trimmed = name_or_id.trim().to_lowercase();
    let mut records = load_custom_wines();
    let initial_len = records.len();
    records.retain(|r| r.id.to_lowercase() != trimmed && r.name.to_lowercase() != trimmed);

    if records.len() < initial_len {
        save_custom_wines(&records)?;

        // If the removed wine was the active selection, reset to primary
        let sel_file = active_wine_selection_file();
        if sel_file.is_file() {
            if let Ok(active_id) = fs::read_to_string(&sel_file) {
                if active_id.trim().to_lowercase() == trimmed {
                    let _ = clear_active_wine();
                }
            }
        }

        Ok(true)
    } else {
        let legacy_file = custom_wine_path_file();
        if trimmed == "custom" && legacy_file.is_file() {
            let _ = fs::remove_file(legacy_file);
            return Ok(true);
        }
        Ok(false)
    }
}

/// Sets a custom Wine runtime path and validates it.
pub fn set_custom_wine_path(path: &Path) -> Result<WineRuntime> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let runtime = add_custom_wine("custom", &canonical)?;

    paths::ensure_dirs()?;
    let _ = fs::write(
        custom_wine_path_file(),
        canonical.to_string_lossy().as_bytes(),
    );

    Ok(runtime)
}

pub fn active_wine_selection_file() -> PathBuf {
    paths::support_dir().join("active_wine.txt")
}

/// Resolves a WineRuntime given an identifier (e.g. "staging", "crossover", "dxmt"),
/// partial name, or direct directory path.
pub fn resolve_wine_runtime_by_query(query: &str) -> Option<WineRuntime> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }

    // 1. Direct path check
    let p = PathBuf::from(q);
    if p.exists() {
        if let Some(rt) = inspect_wine_dir(&p, Some("custom"), Some("Custom Wine")) {
            return Some(rt);
        }
    }

    let runtimes = discover_wine_runtimes();
    let q_lower = q.to_lowercase();

    // 2. Exact ID match (e.g. "staging", "crossover", "staging-dxmt", "brew-staging")
    if let Some(rt) = runtimes.iter().find(|r| r.id.to_lowercase() == q_lower) {
        return Some(rt.clone());
    }

    // 3. Normalized ID match without prefixes/suffixes (e.g. "dxmt" matches "staging-dxmt")
    if let Some(rt) = runtimes
        .iter()
        .find(|r| r.id.to_lowercase().contains(&q_lower))
    {
        return Some(rt.clone());
    }

    // 4. Substring match on name (e.g. "Heroic Wine-CrossOver", "Wine-Staging")
    if let Some(rt) = runtimes
        .iter()
        .find(|r| r.name.to_lowercase().contains(&q_lower))
    {
        return Some(rt.clone());
    }

    None
}

/// Returns the currently active/desired Wine runtime.
/// Priority:
/// 1. NUCLEON_WINE environment variable (e.g. NUCLEON_WINE=crossover)
/// 2. NUCLEON_WINE_PATH environment variable (direct path)
/// 3. Saved selection in active_wine.txt
/// 4. Primary Wine runtime (defaults to Wine-Staging or Heroic Wine)
pub fn get_active_wine_runtime() -> Option<WineRuntime> {
    // 1. Environment variable: NUCLEON_WINE
    if let Ok(query) = std::env::var("NUCLEON_WINE") {
        if let Some(rt) = resolve_wine_runtime_by_query(&query) {
            return Some(rt);
        }
    }

    // 2. Environment variable: NUCLEON_WINE_PATH or WINE_PATH
    for var in &["NUCLEON_WINE_PATH", "WINE_PATH"] {
        if let Ok(path_str) = std::env::var(var) {
            let p = PathBuf::from(path_str);
            if let Some(rt) = inspect_wine_dir(&p, Some("env-wine"), Some("Custom Env Wine")) {
                return Some(rt);
            }
        }
    }

    // 3. Saved selection file
    let sel_file = active_wine_selection_file();
    if sel_file.is_file() {
        if let Ok(content) = fs::read_to_string(&sel_file) {
            let q = content.trim();
            if !q.is_empty() {
                if let Some(rt) = resolve_wine_runtime_by_query(q) {
                    return Some(rt);
                }
            }
        }
    }

    // 4. Fall back to primary runtime
    find_primary_wine_runtime()
}

/// Sets the active Wine runtime by ID, name, or path, persists the selection,
/// and immediately updates the Steam compatibility tool shim if installed.
pub fn set_active_wine(identifier_or_path: &str) -> Result<WineRuntime> {
    let runtime = resolve_wine_runtime_by_query(identifier_or_path)
        .context(format!("Could not find a valid Wine runtime matching '{}'. Run 'nucleon wine list' to view available options.", identifier_or_path))?;

    paths::ensure_dirs()?;
    fs::write(active_wine_selection_file(), runtime.id.as_bytes())
        .with_context(|| format!("Failed to write {}", active_wine_selection_file().display()))?;

    // Also update Steam's nucleon-wine/run shim if it exists so changes take effect immediately
    let steam_wine_run = paths::home_dir()
        .join("Library/Application Support/Steam/compatibilitytools.d/nucleon-wine/run");
    if steam_wine_run.exists() {
        let script = crate::vdf::generate_run_script_full(Some("staging"), Some(&runtime.root));
        let _ = fs::write(&steam_wine_run, script);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&steam_wine_run) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&steam_wine_run, perms);
            }
        }
    }

    log::info!(
        "Set active Wine runtime to '{}' ({})",
        runtime.name,
        runtime.root.display()
    );
    Ok(runtime)
}

/// Clears any configured active Wine selection (reverting to default primary Wine runtime).
pub fn clear_active_wine() -> Result<()> {
    let sel_file = active_wine_selection_file();
    if sel_file.is_file() {
        let _ = fs::remove_file(&sel_file);
    }

    // Update Steam tool shim to primary runtime if present
    if let Some(primary) = find_primary_wine_runtime() {
        let steam_wine_run = paths::home_dir()
            .join("Library/Application Support/Steam/compatibilitytools.d/nucleon-wine/run");
        if steam_wine_run.exists() {
            let script = crate::vdf::generate_run_script_full(Some("staging"), Some(&primary.root));
            let _ = fs::write(&steam_wine_run, script);
        }
    }

    Ok(())
}

/// Clears any configured custom Wine path.
pub fn clear_custom_wine_path() -> Result<()> {
    let _ = remove_custom_wine("custom");
    let custom_file = custom_wine_path_file();
    if custom_file.is_file() {
        let _ = fs::remove_file(&custom_file);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_inspect_wine_dir_standard_layout() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        fs::write(bin_dir.join("wine"), "#!/bin/sh\necho wine-9.0\n").unwrap();
        fs::write(bin_dir.join("wineserver"), "#!/bin/sh\n").unwrap();

        let rt = inspect_wine_dir(dir.path(), Some("test"), Some("Test Wine"));
        assert!(rt.is_some());
        let runtime = rt.unwrap();
        assert_eq!(runtime.id, "test");
        assert_eq!(runtime.name, "Test Wine");
        assert_eq!(runtime.root, dir.path());
    }

    #[test]
    fn test_inspect_wine_dir_bundle_layout() {
        let dir = tempdir().unwrap();
        let inner_wine = dir.path().join("Contents/Resources/wine/bin");
        fs::create_dir_all(&inner_wine).unwrap();

        fs::write(inner_wine.join("wine"), "#!/bin/sh\n").unwrap();
        fs::write(inner_wine.join("wineserver"), "#!/bin/sh\n").unwrap();

        let rt = inspect_wine_dir(dir.path(), Some("bundle"), Some("Bundle Wine"));
        assert!(rt.is_some());
        let runtime = rt.unwrap();
        assert_eq!(runtime.root, dir.path().join("Contents/Resources/wine"));
    }

    #[test]
    fn test_resolve_wine_runtime_by_query_path() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        fs::write(bin_dir.join("wine"), "#!/bin/sh\n").unwrap();
        fs::write(bin_dir.join("wineserver"), "#!/bin/sh\n").unwrap();

        let path_str = dir.path().to_str().unwrap();
        let rt = resolve_wine_runtime_by_query(path_str);
        assert!(rt.is_some());
        assert_eq!(rt.unwrap().root, dir.path());
    }

    #[test]
    fn test_custom_wine_record_serialization() {
        let records = vec![
            CustomWineRecord {
                id: "crossover-24".to_string(),
                name: "CrossOver 24".to_string(),
                path: PathBuf::from("/Applications/CrossOver.app"),
            },
            CustomWineRecord {
                id: "proton-ge".to_string(),
                name: "Proton-GE".to_string(),
                path: PathBuf::from("/opt/proton-ge"),
            },
        ];

        let json = serde_json::to_string(&records).unwrap();
        let deserialized: Vec<CustomWineRecord> = serde_json::from_str(&json).unwrap();
        assert_eq!(records, deserialized);
    }
}
