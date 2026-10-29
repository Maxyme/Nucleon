//! Read-only observations of caches owned by D3DMetal and the application.
//! Discovery is a convention, not proof that a process opened a particular file.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::UNIX_EPOCH,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified_unix_ms: Option<u128>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub root: PathBuf,
    pub state: String,
    pub bytes: u64,
    pub files: Vec<FileInfo>,
    pub errors: Vec<String>,
}
impl Inventory {
    pub fn read(root: PathBuf, unreal: bool) -> Self {
        let mut result = Self {
            root,
            state: "missing".into(),
            bytes: 0,
            files: vec![],
            errors: vec![],
        };
        match fs::symlink_metadata(&result.root) {
            Ok(m) if m.is_dir() => result.state = "present".into(),
            Ok(_) => {
                result.state = "unsupported-path".into();
                return result;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return result,
            Err(e) => {
                result.state = "unreadable".into();
                result.errors.push(e.to_string());
                return result;
            }
        }
        let root = result.root.clone();
        let mut budget = 4096;
        result.walk(&root, if unreal { 0 } else { 4 }, unreal, &mut budget);
        result.files.sort_by(|a, b| a.path.cmp(&b.path));
        if !result.errors.is_empty() {
            result.state = "partial".into();
        }
        result
    }
    fn walk(&mut self, dir: &Path, depth: usize, unreal: bool, budget: &mut usize) {
        let entries = match fs::read_dir(dir) {
            Ok(v) => v,
            Err(e) => {
                self.errors.push(format!("{}: {e}", dir.display()));
                return;
            }
        };
        for entry in entries {
            if *budget == 0 {
                self.errors.push("Inventory entry limit reached".into());
                break;
            }
            *budget -= 1;
            let entry = match entry {
                Ok(v) => v,
                Err(e) => {
                    self.errors.push(e.to_string());
                    continue;
                }
            };
            let path = entry.path();
            let m = match fs::symlink_metadata(&path) {
                Ok(v) => v,
                Err(e) => {
                    self.errors.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            if m.is_dir() {
                if depth > 0 {
                    self.walk(&path, depth - 1, unreal, budget);
                } else if !unreal {
                    self.errors.push(format!("Depth limit: {}", path.display()));
                }
            } else if m.is_file()
                && (!unreal
                    || matches!(
                        path.extension().and_then(|s| s.to_str()),
                        Some("upipelinecache" | "ushaderprecache")
                    ))
            {
                self.bytes = self.bytes.saturating_add(m.len());
                self.files.push(FileInfo {
                    path,
                    bytes: m.len(),
                    modified_unix_ms: m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis()),
                });
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct D3dMetal {
    pub owner: String,
    pub managed_by_nucleon: bool,
    pub discovery: String,
    pub renderer: PathBuf,
    pub runtime: PathBuf,
    pub framework: PathBuf,
    pub framework_binary: Option<FileInfo>,
    pub os_version: Option<String>,
    pub os_build: Option<String>,
    pub process_architecture: String,
    pub cache: Option<Inventory>,
    pub discovery_error: Option<String>,
    pub unreal: Vec<Inventory>,
}
impl D3dMetal {
    pub fn discover(
        runtime: &Path,
        framework: PathBuf,
        renderer: &Path,
        prefix: &Path,
        architecture: &str,
    ) -> Self {
        let system =
            plist::Value::from_file("/System/Library/CoreServices/SystemVersion.plist").ok();
        let value = |key: &str| {
            system
                .as_ref()
                .and_then(|v| v.as_dictionary())
                .and_then(|v| v.get(key))
                .and_then(|v| v.as_string())
                .map(str::to_owned)
        };
        let located = user_cache_dir().and_then(|dir| {
            let name = renderer
                .file_name()
                .ok_or_else(|| "Renderer has no filename".to_owned())?;
            Ok(Inventory::read(
                dir.join("d3dm").join(name).join("shaders.cache"),
                false,
            ))
        });
        let (cache, discovery_error) = match located {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        };
        Self {
            owner: "D3DMetal/application".into(),
            managed_by_nucleon: false,
            discovery: "backend-path-convention; file presence does not prove an active cache hit"
                .into(),
            renderer: renderer.into(),
            runtime: runtime.into(),
            framework_binary: fs::metadata(framework.join("D3DMetal"))
                .ok()
                .map(|m| FileInfo {
                    path: framework.join("D3DMetal"),
                    bytes: m.len(),
                    modified_unix_ms: m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis()),
                }),
            framework,
            os_version: value("ProductVersion"),
            os_build: value("ProductBuildVersion"),
            process_architecture: architecture.into(),
            cache,
            discovery_error,
            unreal: unreal_inventories(prefix, renderer),
        }
    }
    pub fn refresh(&mut self) {
        if let Some(cache) = &mut self.cache {
            *cache = Inventory::read(cache.root.clone(), false);
        }
        for cache in &mut self.unreal {
            *cache = Inventory::read(cache.root.clone(), true);
        }
    }
}
fn user_cache_dir() -> Result<PathBuf, String> {
    let result = Command::new("/usr/bin/getconf")
        .arg("DARWIN_USER_CACHE_DIR")
        .output()
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(format!("getconf DARWIN_USER_CACHE_DIR: {}", result.status));
    }
    let path = PathBuf::from(
        String::from_utf8(result.stdout)
            .map_err(|e| e.to_string())?
            .trim(),
    );
    if !path.is_absolute() {
        return Err("System returned a non-absolute cache path".into());
    }
    Ok(path)
}
fn unreal_inventories(prefix: &Path, renderer: &Path) -> Vec<Inventory> {
    let Some(binaries) = renderer.parent().and_then(Path::parent) else {
        return vec![];
    };
    if binaries.file_name().is_none_or(|n| n != "Binaries") {
        return vec![];
    }
    let Some(game) = binaries.parent().and_then(Path::file_name) else {
        return vec![];
    };
    let Ok(users) = fs::read_dir(prefix.join("drive_c/users")) else {
        return vec![];
    };
    let mut inventories = users
        .take(128)
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path().join("AppData/Local").join(game).join("Saved"))
        .filter(|p| p.is_dir())
        .map(|p| Inventory::read(p, true))
        .collect::<Vec<_>>();
    inventories.sort_by(|a, b| a.root.cmp(&b.root));
    inventories
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_only_inventory_does_not_follow_links_or_create_missing_paths() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        assert_eq!(Inventory::read(missing.clone(), false).state, "missing");
        assert!(!missing.exists());
        let root = temp.path().join("cache");
        fs::create_dir(&root).unwrap();
        let outside = temp.path().join("private");
        fs::write(&outside, b"do not read").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        fs::write(root.join("shader.bin"), b"cached").unwrap();
        let snapshot = Inventory::read(root, false);
        assert_eq!(snapshot.state, "present");
        assert_eq!(snapshot.bytes, 6);
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(fs::read(outside).unwrap(), b"do not read");
    }
    #[test]
    fn unreal_only_reports_pipeline_files_and_preserves_cache_contents() {
        let temp = tempfile::tempdir().unwrap();
        let saved = temp
            .path()
            .join("drive_c/users/user/AppData/Local/Talos2/Saved");
        fs::create_dir_all(&saved).unwrap();
        fs::write(saved.join("game.upipelinecache"), b"PSOs").unwrap();
        fs::write(saved.join("driver.ushaderprecache"), b"driver").unwrap();
        fs::write(saved.join("unrelated.sav"), b"save").unwrap();
        let renderer = Path::new("/Games/Talos2/Binaries/Win64/Talos2.exe");
        let result = unreal_inventories(temp.path(), renderer);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].bytes, 10);
        assert_eq!(result[0].files.len(), 2);
        assert_eq!(fs::read(saved.join("unrelated.sav")).unwrap(), b"save");
    }
}
