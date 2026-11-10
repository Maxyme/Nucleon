#![forbid(unsafe_code)]

use crate::paths;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fmt;
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

/// Returns the configuration file path for the active graphics backend (`backend.toml`).
pub fn backend_config_file() -> PathBuf {
    paths::support_dir().join("backend.toml")
}

/// Strict resolution for launch: never hide an invalid higher-priority setting.
pub fn resolve_active_backend_with_origin() -> Result<(GraphicsBackend, String)> {
    for name in ["NUCLEON_BACKEND", "NUCLEON_GRAPHICS_BACKEND"] {
        if let Some(value) = crate::configuration::environment(name)? {
            let backend = GraphicsBackend::from_str(&value).with_context(|| format!(
                "Invalid {name}={value:?}. Correct it or remove {name} from the launch environment"))?;
            return Ok((backend, format!("from environment variable {name}")));
        }
    }
    let path = backend_config_file();
    if let Some(contents) = crate::configuration::optional_file(&path)? {
        return parse_backend_configuration(&contents, &path)
            .map(|backend| (backend, format!("from config file ({})", path.display())));
    }
    Ok((GraphicsBackend::Auto, "default (auto)".into()))
}

fn parse_backend_configuration(contents: &str, path: &Path) -> Result<GraphicsBackend> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        backend: String,
    }
    let config: Config = crate::configuration::toml(contents, path).with_context(|| format!(
        "Invalid backend configuration {}. Set backend = \"auto\" (or dxvk, dxmt, d3dmetal, kosmickrisp), or remove the file to reset it", path.display()))?;
    GraphicsBackend::from_str(&config.backend).with_context(|| {
        format!(
            "Invalid backend in {}. Correct its backend value or remove the file",
            path.display()
        )
    })
}

/// Read-only status still displays configuration errors when it cannot resolve a backend.
pub fn get_active_backend_with_origin() -> (GraphicsBackend, String) {
    resolve_active_backend_with_origin().unwrap_or_else(|error| {
        log::warn!("Backend configuration failed: {error:#}");
        (
            GraphicsBackend::Auto,
            format!("CONFIGURATION ERROR (Automatic launch blocked): {error:#}"),
        )
    })
}

/// Persists the active graphics backend into `backend.toml`.
pub fn set_active_backend(backend: GraphicsBackend) -> Result<()> {
    paths::ensure_dirs()?;
    let conf_path = backend_config_file();
    let content = format!(
        "# Nucleon Graphics Backend Configuration\n# Options: auto, dxvk, dxmt, d3dmetal, kosmickrisp\nbackend = \"{}\"\n",
        backend.as_str()
    );
    crate::fs_util::atomic_write_file(&conf_path, content.as_bytes())
        .with_context(|| format!("Failed to write backend config to {}", conf_path.display()))?;
    log::info!(
        "Persisted active graphics backend '{}' to {}",
        backend.as_str(),
        conf_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn backend_configuration_uses_toml_and_rejects_invalid_settings() {
        let path = Path::new("/tmp/example/backend.toml");
        assert_eq!(
            parse_backend_configuration("backend = 'dxvk' # chosen backend", path).unwrap(),
            GraphicsBackend::Dxvk
        );
        for contents in [
            "",
            "backend = 'typo'",
            "backend = 1",
            "backed = 'auto'",
            "backend = '",
        ] {
            let error = format!(
                "{:#}",
                parse_backend_configuration(contents, path).unwrap_err()
            );
            assert!(error.contains("/tmp/example/backend.toml"));
            assert!(error.contains("remove the file"));
        }
    }

    #[test]
    fn test_backend_parsing_and_display() {
        assert_eq!(
            GraphicsBackend::from_str("auto").unwrap(),
            GraphicsBackend::Auto
        );
        assert_eq!(
            GraphicsBackend::from_str("dxvk").unwrap(),
            GraphicsBackend::Dxvk
        );
        assert_eq!(
            GraphicsBackend::from_str("dxmt").unwrap(),
            GraphicsBackend::Dxmt
        );
        assert_eq!(
            GraphicsBackend::from_str("d3dmetal").unwrap(),
            GraphicsBackend::D3DMetal
        );
        assert_eq!(
            GraphicsBackend::from_str("gptk").unwrap(),
            GraphicsBackend::D3DMetal
        );
        assert_eq!(
            GraphicsBackend::from_str("kosmickrisp").unwrap(),
            GraphicsBackend::KosmicKrisp
        );
        assert_eq!(
            GraphicsBackend::from_str("vulkan").unwrap(),
            GraphicsBackend::KosmicKrisp
        );
        assert!(GraphicsBackend::from_str("invalid").is_err());
    }

    #[test]
    fn test_backend_persistence_roundtrip() {
        let temp = tempdir().unwrap();
        let conf_file = temp.path().join("backend.toml");

        // Write config
        let content = "backend = \"dxmt\"\n";
        fs::write(&conf_file, content).unwrap();

        let resolved =
            parse_backend_configuration(&fs::read_to_string(&conf_file).unwrap(), &conf_file)
                .unwrap();
        assert_eq!(resolved, GraphicsBackend::Dxmt);
    }
}
