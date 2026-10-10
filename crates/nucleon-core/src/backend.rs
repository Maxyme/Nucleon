#![forbid(unsafe_code)]

use crate::paths;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Available graphical translation backends for Windows games on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsBackend {
    /// Automatic engine / backend selection based on executable inspection and defaults
    Auto,
    /// DXVK: Direct3D 9/10/11 -> Vulkan 1.4
    Dxvk,
    /// DXMT: Direct3D 11 -> Apple Metal
    Dxmt,
    /// Apple D3DMetal (GPTK): Direct3D 11/12 -> Apple Metal
    #[serde(rename = "d3dmetal")]
    D3DMetal,
    /// Mesa KosmicKrisp: Khronos-conformant Vulkan 1.4 driver on Metal 4
    #[serde(rename = "kosmickrisp")]
    KosmicKrisp,
}

impl GraphicsBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dxvk => "dxvk",
            Self::Dxmt => "dxmt",
            Self::D3DMetal => "d3dmetal",
            Self::KosmicKrisp => "kosmickrisp",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Auto => "Auto (Game-tailored / WineD3D)",
            Self::Dxvk => "DXVK (Direct3D 9/10/11 -> Vulkan)",
            Self::Dxmt => "DXMT (Direct3D 11 -> Apple Metal)",
            Self::D3DMetal => "Apple D3DMetal (Direct3D 11/12 -> Apple Metal)",
            Self::KosmicKrisp => "Mesa KosmicKrisp (Vulkan 1.4)",
        }
    }

    pub fn all() -> &'static [GraphicsBackend] {
        &[
            Self::Auto,
            Self::Dxvk,
            Self::Dxmt,
            Self::D3DMetal,
            Self::KosmicKrisp,
        ]
    }
}

impl fmt::Display for GraphicsBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for GraphicsBackend {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let clean = s.trim().to_lowercase();
        match clean.as_str() {
            "auto" => Ok(Self::Auto),
            "dxvk" => Ok(Self::Dxvk),
            "dxmt" => Ok(Self::Dxmt),
            "d3dmetal" | "gptk" | "gptk4" | "d3dm" => Ok(Self::D3DMetal),
            "kosmickrisp" | "vulkan" | "mesa" => Ok(Self::KosmicKrisp),
            _ => bail!(
                "Unknown graphics backend '{}'. Valid options: auto, dxvk, dxmt, d3dmetal, kosmickrisp",
                s
            ),
        }
    }
}

/// Status summary for a graphics backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendStatus {
    pub backend: GraphicsBackend,
    pub installed: bool,
    pub version: Option<String>,
    pub location: Option<PathBuf>,
    pub supported_dx: &'static str,
    pub description: String,
}

/// Common trait implemented by each graphics translation backend.
pub trait Backend {
    fn backend(&self) -> GraphicsBackend;
    fn name(&self) -> &'static str {
        self.backend().as_str()
    }
    fn display_name(&self) -> &'static str {
        self.backend().display_name()
    }
    /// Returns the supported DirectX version range (e.g. "DX9-DX11", "DX10-DX11", "DX11/DX12").
    fn supported_dx_versions(&self) -> &'static str;
    /// Inspects system status, version, and location of this backend.
    fn status(&self) -> BackendStatus;
    /// Validates and registers a custom installation path for this backend.
    fn install(&self, paths: &Path) -> Result<()>;
    /// Enables this backend for a Wine prefix, staging translation DLLs as needed.
    fn enable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize>;
    /// Disables this backend for a Wine prefix, unstaging DLLs and restoring builtins.
    fn disable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize>;
    /// Assembles environment variables required for execution under this backend.
    fn apply_env(
        &self,
        env: &mut HashMap<String, String>,
        runner_dir: &Path,
        prefix_dir: &Path,
        steam_dir: &Path,
        client_path_str: &str,
        enable_hud: bool,
    );
}

/// Returns the configuration file path for the active graphics backend (`backend.toml`).
pub fn backend_config_file() -> PathBuf {
    paths::support_dir().join("backend.toml")
}

/// Resolves the currently active graphics backend along with its resolution origin.
pub fn get_active_backend_with_origin() -> (GraphicsBackend, String) {
    // 1. Environment variable: NUCLEON_BACKEND or NUCLEON_GRAPHICS_BACKEND
    for var in &["NUCLEON_BACKEND", "NUCLEON_GRAPHICS_BACKEND"] {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                if let Ok(b) = GraphicsBackend::from_str(trimmed) {
                    return (b, format!("from environment variable {var}"));
                }
            }
        }
    }

    // 2. Persisted config file: backend.toml
    let conf_path = backend_config_file();
    if conf_path.is_file() {
        if let Ok(content) = fs::read_to_string(&conf_path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('#') || trimmed.is_empty() {
                    continue;
                }
                if let Some((k, v)) = trimmed.split_once('=') {
                    if k.trim() == "backend" {
                        let clean_val = v.trim().trim_matches('"').trim_matches('\'');
                        if let Ok(b) = GraphicsBackend::from_str(clean_val) {
                            return (b, format!("from config file ({})", conf_path.display()));
                        }
                    }
                }
            }
        }
    }

    // 3. Default fallback
    (GraphicsBackend::Auto, "default (auto)".to_string())
}

/// Returns the currently active graphics backend.
pub fn get_active_backend() -> GraphicsBackend {
    get_active_backend_with_origin().0
}

/// Persists the active graphics backend into `backend.toml`.
pub fn set_active_backend(backend: GraphicsBackend) -> Result<()> {
    paths::ensure_dirs()?;
    let conf_path = backend_config_file();
    let content = format!(
        "# Nucleon Graphics Backend Configuration\n# Options: auto, dxvk, dxmt, d3dmetal, kosmickrisp\nbackend = \"{}\"\n",
        backend.as_str()
    );
    fs::write(&conf_path, content.as_bytes())
        .with_context(|| format!("Failed to write backend config to {}", conf_path.display()))?;
    log::info!("Persisted active graphics backend '{}' to {}", backend.as_str(), conf_path.display());
    Ok(())
}

/// Factory function returning the concrete `Backend` implementation for a given enum variant.
pub fn create_backend(backend: GraphicsBackend) -> Box<dyn Backend> {
    match backend {
        GraphicsBackend::Auto => Box::new(AutoBackend),
        GraphicsBackend::Dxvk => Box::new(crate::dxvk::DxvkBackend),
        GraphicsBackend::Dxmt => Box::new(crate::dxmt::DxmtBackend),
        GraphicsBackend::D3DMetal => Box::new(D3DMetalBackend),
        GraphicsBackend::KosmicKrisp => Box::new(KosmicKrispBackend),
    }
}

/// Auto backend adapter that resolves dynamically based on detected availability.
pub struct AutoBackend;

impl Backend for AutoBackend {
    fn backend(&self) -> GraphicsBackend {
        GraphicsBackend::Auto
    }

    fn supported_dx_versions(&self) -> &'static str {
        "Auto (DX9-DX12)"
    }

    fn status(&self) -> BackendStatus {
        BackendStatus {
            backend: GraphicsBackend::Auto,
            installed: true,
            version: None,
            location: None,
            supported_dx: "Auto (DX9-DX12)",
            description: "Automatically routes graphics translation based on detected target executable and configured runtimes".to_string(),
        }
    }

    fn install(&self, _paths: &Path) -> Result<()> {
        bail!("Cannot install path directly for 'auto' backend. Configure specific backend (dxvk, dxmt, d3dmetal, kosmickrisp).")
    }

    fn enable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize> {
        self.disable(prefix, runner_dir)
    }

    fn disable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize> {
        let mut count = 0;
        count += crate::dxvk::unstage_dxvk_from_prefix(prefix, runner_dir)?;
        count += crate::dxmt::unstage_dxmt_from_prefix(prefix, runner_dir)?;
        count += crate::vkd3d::unstage_vkd3d_proton_from_prefix(prefix, runner_dir)?;
        count += crate::d7vk::unstage_d7vk_from_prefix(prefix, runner_dir)?;
        Ok(count)
    }

    fn apply_env(
        &self,
        env: &mut HashMap<String, String>,
        runner_dir: &Path,
        prefix_dir: &Path,
        steam_dir: &Path,
        client_path_str: &str,
        enable_hud: bool,
    ) {
        // Fallback to Wine-Staging legacy WineD3D / OpenGL
        let _ = (runner_dir, prefix_dir, steam_dir, client_path_str);
        if enable_hud {
            env.insert("MTL_HUD_ENABLED".to_string(), "1".to_string());
        }
    }
}

/// D3DMetal (Apple GPTK 4) backend implementation.
pub struct D3DMetalBackend;

impl Backend for D3DMetalBackend {
    fn backend(&self) -> GraphicsBackend {
        GraphicsBackend::D3DMetal
    }

    fn supported_dx_versions(&self) -> &'static str {
        "DX11-DX12"
    }

    fn status(&self) -> BackendStatus {
        let (installed, location, version) = match crate::runner::find_gptk_components(None) {
            Ok(Some((fw, _))) => {
                let ver = crate::runner::detect_gptk_version(None);
                (true, Some(fw), ver)
            }
            _ => (false, None, None),
        };

        BackendStatus {
            backend: GraphicsBackend::D3DMetal,
            installed,
            version,
            location,
            supported_dx: "DX11-DX12",
            description: "Apple Game Porting Toolkit 4 Direct3D 11 & 12 translation directly to Apple Metal".to_string(),
        }
    }

    fn install(&self, paths: &Path) -> Result<()> {
        crate::runner::set_custom_gptk_path(paths).map(|_| ())
    }

    fn enable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize> {
        // Unstage Vulkan/DXMT translation layers to allow native D3DMetal execution
        let mut count = 0;
        count += crate::dxvk::unstage_dxvk_from_prefix(prefix, runner_dir)?;
        count += crate::dxmt::unstage_dxmt_from_prefix(prefix, runner_dir)?;
        count += crate::vkd3d::unstage_vkd3d_proton_from_prefix(prefix, runner_dir)?;
        count += crate::d7vk::unstage_d7vk_from_prefix(prefix, runner_dir)?;
        Ok(count)
    }

    fn disable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize> {
        self.enable(prefix, runner_dir)
    }

    fn apply_env(
        &self,
        env: &mut HashMap<String, String>,
        runner_dir: &Path,
        _prefix_dir: &Path,
        steam_dir: &Path,
        client_path_str: &str,
        enable_hud: bool,
    ) {
        crate::runner::apply_gptk_execution_env(env, runner_dir, steam_dir, client_path_str, enable_hud);
    }
}

/// Mesa KosmicKrisp Vulkan backend implementation.
pub struct KosmicKrispBackend;

impl Backend for KosmicKrispBackend {
    fn backend(&self) -> GraphicsBackend {
        GraphicsBackend::KosmicKrisp
    }

    fn supported_dx_versions(&self) -> &'static str {
        "DX9-DX12 (Vulkan 1.4)"
    }

    fn status(&self) -> BackendStatus {
        let (installed, location, version) = match crate::runner::get_kosmickrisp_info() {
            Some(info) => (true, Some(info.icd_path), Some(info.api_version)),
            None => (false, None, None),
        };

        BackendStatus {
            backend: GraphicsBackend::KosmicKrisp,
            installed,
            version,
            location,
            supported_dx: "DX9-DX12 (Vulkan 1.4)",
            description: "Mesa KosmicKrisp Khronos-conformant Vulkan 1.4 driver on Apple Metal 4".to_string(),
        }
    }

    fn install(&self, paths: &Path) -> Result<()> {
        crate::runner::set_custom_kosmickrisp_path(paths, None).map(|_| ())
    }

    fn enable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize> {
        let mut count = 0;
        // DXMT conflicts with DXVK, so unstage DXMT
        let _ = crate::dxmt::unstage_dxmt_from_prefix(prefix, runner_dir);

        if let Some(vkd3d) = crate::vkd3d::find_vkd3d_proton() {
            if let Ok(staged) = crate::vkd3d::stage_vkd3d_proton_into_prefix(&vkd3d, prefix) {
                count += staged;
            }
        }
        if let Some(d7vk) = crate::d7vk::find_d7vk() {
            if let Ok(staged) = crate::d7vk::stage_d7vk_into_prefix(&d7vk, prefix) {
                count += staged;
            }
        }
        if let Some(dxvk) = crate::dxvk::find_dxvk() {
            if let Ok(staged) = crate::dxvk::stage_dxvk_into_prefix(&dxvk, prefix) {
                count += staged;
            }
        }
        Ok(count)
    }

    fn disable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize> {
        let mut count = 0;
        count += crate::vkd3d::unstage_vkd3d_proton_from_prefix(prefix, runner_dir)?;
        count += crate::d7vk::unstage_d7vk_from_prefix(prefix, runner_dir)?;
        count += crate::dxvk::unstage_dxvk_from_prefix(prefix, runner_dir)?;
        Ok(count)
    }

    fn apply_env(
        &self,
        env: &mut HashMap<String, String>,
        runner_dir: &Path,
        _prefix_dir: &Path,
        steam_dir: &Path,
        client_path_str: &str,
        enable_hud: bool,
    ) {
        if let Some(icd) = crate::runner::find_kosmickrisp_icd() {
            env.insert("VK_DRIVER_FILES".to_string(), icd.to_string_lossy().to_string());
            env.insert("VK_ICD_FILENAMES".to_string(), icd.to_string_lossy().to_string());
        }
        env.insert("MESA_LOADER_DRIVER_OVERRIDE".to_string(), "kosmickrisp".to_string());

        if std::env::var("GALLIUM_DRIVER").is_err() {
            env.insert("GALLIUM_DRIVER".to_string(), "zink".to_string());
        }
        if std::env::var("MESA_GL_VERSION_OVERRIDE").is_err() {
            env.insert("MESA_GL_VERSION_OVERRIDE".to_string(), "4.6".to_string());
        }
        if std::env::var("MESA_GLSL_VERSION_OVERRIDE").is_err() {
            env.insert("MESA_GLSL_VERSION_OVERRIDE".to_string(), "460".to_string());
        }

        let shim_dir = crate::runner::setup_kosmickrisp_shim().unwrap_or_else(|_| paths::kosmickrisp_shim_dir());

        let mut overrides = "steamclient=n,b;steamclient64=n,b;lsteamclient=b;winevulkan=b,n;vulkan-1=b,n;d3d12,d3d12core=n,b".to_string();
        if crate::dxvk::find_dxvk().is_some() {
            overrides.push_str(";d3d11,dxgi,d3d10core,d3d9=n,b");
        }
        if crate::d7vk::find_d7vk().is_some() {
            overrides.push_str(";ddraw=n,b");
        }
        env.insert("WINEDLLOVERRIDES".to_string(), overrides);

        if crate::dxvk::find_dxvk().is_some() {
            env.insert("DXVK_LOG_LEVEL".to_string(), "info".to_string());
            env.insert("DXVK_LOG_PATH".to_string(), paths::support_dir().to_string_lossy().to_string());
        }

        if crate::vkd3d::find_vkd3d_proton().is_some() {
            env.insert("VKD3D_CONFIG".to_string(), "dxr11,dxr".to_string());
        }

        let mesa_cache = paths::home_dir().join("Library/Caches/Mesa");
        let _ = fs::create_dir_all(&mesa_cache);
        env.insert("MESA_SHADER_CACHE_DIR".to_string(), mesa_cache.to_string_lossy().to_string());

        if enable_hud {
            env.insert("MTL_HUD_ENABLED".to_string(), "1".to_string());
            env.insert("VKD3D_DEBUG".to_string(), "warn".to_string());
        }

        let lib_dir = runner_dir.join("lib");
        let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
        env.insert(
            "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
            format!(
                "{}:{}:{}:{}:{}",
                steam_dir.display(),
                client_path_str,
                shim_dir.display(),
                lib_unix.display(),
                lib_dir.display()
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_backend_parsing_and_display() {
        assert_eq!(GraphicsBackend::from_str("auto").unwrap(), GraphicsBackend::Auto);
        assert_eq!(GraphicsBackend::from_str("dxvk").unwrap(), GraphicsBackend::Dxvk);
        assert_eq!(GraphicsBackend::from_str("dxmt").unwrap(), GraphicsBackend::Dxmt);
        assert_eq!(GraphicsBackend::from_str("d3dmetal").unwrap(), GraphicsBackend::D3DMetal);
        assert_eq!(GraphicsBackend::from_str("gptk").unwrap(), GraphicsBackend::D3DMetal);
        assert_eq!(GraphicsBackend::from_str("kosmickrisp").unwrap(), GraphicsBackend::KosmicKrisp);
        assert_eq!(GraphicsBackend::from_str("vulkan").unwrap(), GraphicsBackend::KosmicKrisp);
        assert!(GraphicsBackend::from_str("invalid").is_err());
    }

    #[test]
    fn test_backend_persistence_roundtrip() {
        let temp = tempdir().unwrap();
        let conf_file = temp.path().join("backend.toml");

        // Write config
        let content = "backend = \"dxmt\"\n";
        fs::write(&conf_file, content).unwrap();

        // Read line
        let mut resolved = None;
        for line in fs::read_to_string(&conf_file).unwrap().lines() {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "backend" {
                    resolved = Some(GraphicsBackend::from_str(v.trim().trim_matches('"')).unwrap());
                }
            }
        }
        assert_eq!(resolved, Some(GraphicsBackend::Dxmt));
    }
}
