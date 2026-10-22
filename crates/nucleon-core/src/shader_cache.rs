//! Persistent per-game graphics caches, independent of Wine generation paths.
use crate::{
    configuration, detector::TargetEngine, launch::LaunchPlan, runtime::ExecutionMode, staging,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
pub struct ShaderCaches {
    pub root: PathBuf,
    pub compatibility: String,
    pub vkd3d: Option<PathBuf>,
    pub dxvk: Option<PathBuf>,
    pub mesa: PathBuf,
    /// Actual environment paths, including deliberate caller overrides.
    pub environment: BTreeMap<String, String>,
}
impl ShaderCaches {
    pub fn prepare(
        plan: &LaunchPlan,
        compat_data: &Path,
        appid: Option<&str>,
        exe: &Path,
        environment: &HashMap<String, String>,
    ) -> Result<Option<Self>> {
        let engine = match plan.engine {
            TargetEngine::Dxvk => "dxvk",
            TargetEngine::Vkd3d => "vkd3d",
            TargetEngine::KosmicKrisp => "kosmickrisp",
            _ => return Ok(None),
        };
        let mut inputs = Vec::new();
        if plan.engine != TargetEngine::Dxvk
            && let Some(bundle) = &plan.graphics.vkd3d
        {
            if plan.is64 {
                inputs.push(bundle.x64_d3d12.clone());
                inputs.extend(bundle.x64_d3d12core.clone());
            } else {
                inputs.extend(bundle.x86_d3d12.clone());
                inputs.extend(bundle.x86_d3d12core.clone());
            }
        }
        if let Some(bundle) = &plan.graphics.dxvk {
            let files = if plan.is64 {
                [
                    &bundle.x64_d3d11,
                    &bundle.x64_d3d10core,
                    &bundle.x64_d3d9,
                    &bundle.x64_dxgi,
                ]
            } else {
                [
                    &bundle.x86_d3d11,
                    &bundle.x86_d3d10core,
                    &bundle.x86_d3d9,
                    &bundle.x86_dxgi,
                ]
            };
            inputs.extend(files.into_iter().flatten().cloned());
        }
        if !plan.is64
            && plan.engine != TargetEngine::Dxvk
            && let Some(bundle) = &plan.graphics.d7vk
        {
            inputs.push(bundle.x86_ddraw.clone());
        }
        inputs.extend(
            plan.graphics
                .vulkan
                .iter()
                .map(|driver| driver.library.clone()),
        );
        let tuning = crate::launch::FEATURE_OVERRIDES
            .iter()
            .filter_map(|key| environment.get(*key).map(|v| (*key, v)))
            .collect::<BTreeMap<_, _>>();
        let namespace = staging::key(&("graphics-cache-v1", engine, &tuning))?;
        let compatibility = staging::source_key(&namespace, &inputs)?;
        let game = appid
            .filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()))
            .map(str::to_owned)
            .unwrap_or(format!(
                "manual-{}",
                staging::key(&std::path::absolute(exe)?)?
            ));
        plan.runtime.prefix.validate()?;
        let root = std::path::absolute(compat_data)?
            .join("nucleon/shader-cache/v1")
            .join(game)
            .join(match plan.runtime.mode {
                ExecutionMode::Arm64Fex => "arm64-fex",
                ExecutionMode::Intel => "intel",
            })
            .join(format!(
                "{}-abi{}",
                plan.runtime.prefix.family, plan.runtime.prefix.abi
            ))
            .join(if plan.is64 { "x64" } else { "x86" })
            .join(engine)
            .join(&compatibility);
        let vkd3d = (plan.engine != TargetEngine::Dxvk && plan.graphics.vkd3d.is_some())
            .then(|| root.join("vkd3d"));
        let dxvk = plan.graphics.dxvk.is_some().then(|| root.join("dxvk"));
        let mesa = root.join("mesa");
        let mut selected = BTreeMap::new();
        for (key, path, windows) in [
            ("VKD3D_SHADER_CACHE_PATH", vkd3d.as_deref(), true),
            ("DXVK_SHADER_CACHE_PATH", dxvk.as_deref(), true),
            ("MESA_SHADER_CACHE_DIR", Some(mesa.as_path()), false),
        ] {
            if let Some(path) = path {
                let value = match configuration::environment(key)? {
                    Some(value) => {
                        anyhow::ensure!(
                            !value.is_empty(),
                            "{key} must name a cache directory; remove the override to use Nucleon's managed cache"
                        );
                        value
                    }
                    None => {
                        fs::create_dir_all(path)
                            .with_context(|| format!("Create shader cache {}", path.display()))?;
                        let path = path.canonicalize()?;
                        if windows {
                            short_windows_path(&plan.prefix, &root, &path)?
                        } else {
                            path.display().to_string()
                        }
                    }
                };
                selected.insert(key.into(), value);
            }
        }
        Ok(Some(Self {
            root,
            compatibility,
            vkd3d,
            dxvk,
            mesa,
            environment: selected,
        }))
    }
    pub fn apply(&self, environment: &mut HashMap<String, String>) {
        environment.extend(self.environment.clone());
    }
}

fn short_windows_path(prefix: &Path, root: &Path, path: &Path) -> Result<String> {
    use std::os::unix::fs::symlink;
    let root = root.canonicalize()?;
    let relative = path
        .strip_prefix(&root)
        .context("Cache path is outside its namespace")?;
    let namespace = staging::key(&root)?;
    let parent = prefix.join("drive_c/nucleon-shader-cache");
    fs::create_dir_all(&parent)?;
    let alias = parent.join(&namespace);
    match fs::symlink_metadata(&alias) {
        Ok(metadata) => anyhow::ensure!(
            metadata.file_type().is_symlink() && alias.canonicalize()? == root,
            "Managed shader-cache alias conflicts with an existing path: {}",
            alias.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => symlink(&root, &alias)?,
        Err(error) => return Err(error.into()),
    }
    Ok(format!(
        "C:\\nucleon-shader-cache\\{}\\{}",
        namespace,
        relative.display().to_string().replace('/', "\\")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        launch::{GraphicsComponents, VulkanDriver},
        runtime::{self, ExecutionMode},
    };
    #[test]
    fn long_host_cache_paths_use_short_aliases_and_preserve_existing_entries() {
        let temp = tempfile::tempdir().unwrap();
        let mut root = temp.path().to_path_buf();
        for _ in 0..6 {
            root.push("long-host-directory-for-a-persistent-game-cache");
        }
        root.push("namespace");
        let cache = root.join("dxvk");
        fs::create_dir_all(&cache).unwrap();
        let prefix = temp.path().join("prefix");
        let path = short_windows_path(&prefix, &root, &cache.canonicalize().unwrap()).unwrap();
        assert!(cache.as_os_str().len() > 260);
        assert!(path.len() + 32 < 260);
        let alias = prefix
            .join("drive_c")
            .join(path.strip_prefix("C:\\").unwrap().replace('\\', "/"));
        fs::write(alias.join("shader.bin"), b"persistent cached bytes").unwrap();
        assert_eq!(
            fs::read(cache.join("shader.bin")).unwrap(),
            b"persistent cached bytes"
        );
        assert_eq!(
            short_windows_path(&prefix, &root, &cache.canonicalize().unwrap()).unwrap(),
            path
        );
        let link = alias.parent().unwrap();
        fs::remove_file(link).unwrap();
        fs::create_dir(link).unwrap();
        fs::write(link.join("user-file"), b"preserve").unwrap();
        assert!(short_windows_path(&prefix, &root, &cache.canonicalize().unwrap()).is_err());
        assert_eq!(fs::read(link.join("user-file")).unwrap(), b"preserve");
    }

    #[test]
    fn compatible_wine_moves_keep_cached_bytes_and_changed_graphics_or_abi_use_new_namespaces() {
        if crate::test_support::run_in_isolated_support(
            "shader_cache::tests::compatible_wine_moves_keep_cached_bytes_and_changed_graphics_or_abi_use_new_namespaces",
        ) {
            return;
        }
        let (temp, mut runtime) = runtime::fixture(ExecutionMode::Intel);
        let source = temp.path().join("dxvk");
        fs::create_dir(&source).unwrap();
        for name in ["d3d11.dll", "dxgi.dll"] {
            fs::write(source.join(name), b"translator").unwrap();
        }
        let driver = temp.path().join("driver.dylib");
        fs::write(&driver, b"driver").unwrap();
        let mut plan = LaunchPlan {
            runtime_features: Default::default(),
            runtime: runtime.clone(),
            prefix: temp.path().join("pfx"),
            engine: TargetEngine::Dxvk,
            graphics: GraphicsComponents {
                dxvk: crate::dxvk::inspect_candidate_dir(&source),
                vulkan: vec![VulkanDriver {
                    manifest: temp.path().join("icd.json"),
                    library: driver.clone(),
                }],
                ..Default::default()
            },
            bridge: temp.path().join("bridge"),
            is64: true,
            use_steam_helper: false,
            log_directory: None,
            diagnostics: false,
        };
        let compat = temp.path().join("compatdata/123");
        let empty = HashMap::new();
        let cache =
            ShaderCaches::prepare(&plan, &compat, Some("123"), Path::new("game.exe"), &empty)
                .unwrap()
                .unwrap();
        let warmed = cache.dxvk.as_ref().unwrap().join("shader.cache");
        fs::write(&warmed, b"warm").unwrap();
        runtime.root = temp.path().join("new-wine-generation");
        plan.runtime = runtime;
        let again =
            ShaderCaches::prepare(&plan, &compat, Some("123"), Path::new("game.exe"), &empty)
                .unwrap()
                .unwrap();
        assert_eq!(cache.root, again.root);
        assert_eq!(fs::read(&warmed).unwrap(), b"warm");
        fs::write(&driver, b"new driver").unwrap();
        let changed =
            ShaderCaches::prepare(&plan, &compat, Some("123"), Path::new("game.exe"), &empty)
                .unwrap()
                .unwrap();
        assert_ne!(cache.root, changed.root);
        plan.runtime.prefix.abi += 1;
        let changed_abi =
            ShaderCaches::prepare(&plan, &compat, Some("123"), Path::new("game.exe"), &empty)
                .unwrap()
                .unwrap();
        assert_ne!(changed.root, changed_abi.root);
        fs::write(source.join("d3d11.dll"), b"new translator").unwrap();
        let changed_translator =
            ShaderCaches::prepare(&plan, &compat, Some("123"), Path::new("game.exe"), &empty)
                .unwrap()
                .unwrap();
        assert_ne!(changed_abi.root, changed_translator.root);
        let tuning = HashMap::from([("VKD3D_CONFIG".into(), "dxr".into())]);
        let tuned =
            ShaderCaches::prepare(&plan, &compat, Some("123"), Path::new("game.exe"), &tuning)
                .unwrap()
                .unwrap();
        assert_ne!(changed_translator.root, tuned.root);
        let other_game =
            ShaderCaches::prepare(&plan, &compat, Some("456"), Path::new("game.exe"), &empty)
                .unwrap()
                .unwrap();
        assert_ne!(changed_translator.root, other_game.root);
        assert!(warmed.exists());
        assert!(
            cache.environment["DXVK_SHADER_CACHE_PATH"].starts_with("C:\\nucleon-shader-cache\\")
        );
        assert!(cache.environment["MESA_SHADER_CACHE_DIR"].starts_with('/'));
    }
}
