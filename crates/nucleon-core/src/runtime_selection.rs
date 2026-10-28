//! Explicit runtime requests. Emulator choices select complete compatible Wine builds.
use crate::{
    configuration, paths,
    runtime::{ExecutionMode, ResolvedRuntime},
    wine,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Emulator {
    Rosetta,
    Fex,
}

impl Emulator {
    pub fn mode(self) -> ExecutionMode {
        match self {
            Self::Rosetta => ExecutionMode::Intel,
            Self::Fex => ExecutionMode::Arm64Fex,
        }
    }
    pub fn check(self, runtime: &ResolvedRuntime) -> Result<()> {
        ensure!(
            runtime.mode == self.mode(),
            "Requested emulator {self}, but Wine {} uses {:?}; choose a compatible Wine or remove --emulator",
            runtime.root.display(),
            runtime.mode
        );
        Ok(())
    }
}
impl fmt::Display for Emulator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rosetta => "rosetta",
            Self::Fex => "fex",
        })
    }
}
impl FromStr for Emulator {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "rosetta" => Ok(Self::Rosetta),
            "fex" => Ok(Self::Fex),
            _ => anyhow::bail!("Unknown emulator {value:?}; choose rosetta or fex"),
        }
    }
}

pub fn fex_default() -> Result<ResolvedRuntime> {
    load_fex_selection(&fex_default_file())
        .context("No usable FEX default. Run 'nucleon wine fex select <runtime>'")
}

pub fn fex_default_file() -> std::path::PathBuf {
    paths::support_dir().join("fex-runtime.json")
}

/// Store a validated runtime identity independently of Steam.
pub fn set_fex_default(runtime: &ResolvedRuntime) -> Result<()> {
    Emulator::Fex.check(runtime)?;
    std::fs::create_dir_all(paths::support_dir())?;
    crate::fs_util::atomic_write_file(
        fex_default_file(),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1, "root": runtime.root, "prefix": runtime.prefix,
            "managed": runtime.root.join(crate::managed_runtime::RECEIPT).is_file(),
        }))?,
    )
}

/// Prefer the deliberately selected native default; a broken selection is an error.
pub fn standalone_default() -> Result<ResolvedRuntime> {
    if fex_default_file().try_exists()? {
        fex_default()
    } else {
        rosetta_default()
    }
}

/// Exact names and paths only: a version substring must not change execution mode.
pub fn named(query: &str) -> Result<ResolvedRuntime> {
    ensure!(
        !query.trim().is_empty(),
        "Wine runtime cannot be empty; run 'nucleon wine list'"
    );
    let path = Path::new(query);
    if path.exists() {
        let root = wine::resolve_wine_runtime_root(path)
            .context("Wine loader/server were not found at the requested path")?;
        return ResolvedRuntime::inspect(&root);
    }
    // Path-like requests never fall through to name discovery or execute other Wines.
    ensure!(
        !query.contains('/') && !query.contains('\\'),
        "Wine runtime path does not exist: {query}"
    );
    let candidates = wine::discover_wine_runtimes();
    let matches: Vec<_> = candidates
        .iter()
        .filter(|r| r.id.eq_ignore_ascii_case(query) || r.name.eq_ignore_ascii_case(query))
        .collect();
    ensure!(
        matches.len() == 1,
        "Wine runtime {query:?} was not found or is ambiguous; run 'nucleon wine list' and use an exact ID or path"
    );
    matches[0].capabilities()
}

pub fn rosetta_default() -> Result<ResolvedRuntime> {
    let saved = wine::active_wine_selection_file();
    let runtime = if let Some(query) = configuration::optional_file(&saved)? {
        named(query.trim())
            .with_context(|| format!("Invalid saved Wine selection {}", saved.display()))?
    } else if let Some(path) = configuration::optional_file(&wine::custom_wine_path_file())? {
        named(path.trim())?
    } else {
        wine::find_primary_wine_runtime()
            .context("No Intel Wine default; install Wine and run 'nucleon wine use <runtime>'")?
            .capabilities()?
    };
    Emulator::Rosetta.check(&runtime)?;
    Ok(runtime)
}

pub fn resolve(wine: Option<&str>, emulator: Option<Emulator>) -> Result<Option<ResolvedRuntime>> {
    let runtime = match (wine, emulator) {
        (Some(query), _) => named(query)?,
        (None, Some(Emulator::Fex)) => fex_default()?,
        (None, Some(Emulator::Rosetta)) => rosetta_default()?,
        (None, None) => return Ok(None),
    };
    if let Some(emulator) = emulator {
        emulator.check(&runtime)?;
    }
    Ok(Some(runtime))
}

/// Check host translation before prefix preparation; never install Rosetta implicitly.
pub fn check_host(runtime: &ResolvedRuntime) -> Result<()> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if runtime.mode == ExecutionMode::Intel {
        let status = std::process::Command::new("/usr/bin/arch")
            .args(["-x86_64", "/usr/bin/true"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        ensure!(
            status.success(),
            "Rosetta is unavailable. Install Rosetta to use Intel Wine, or select a complete FEX runtime"
        );
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    let _ = runtime;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_selection_pins_runtime_and_rejects_invalid_configuration() {
        if crate::test_support::run_in_isolated_support(
            "runtime_selection::tests::default_selection_pins_runtime_and_rejects_invalid_configuration",
        ) {
            return;
        }
        let (_temp, runtime) = crate::runtime::fixture(ExecutionMode::Arm64Fex);
        set_fex_default(&runtime).unwrap();
        assert_eq!(standalone_default().unwrap().root, runtime.root);
        std::fs::write(fex_default_file(), b"broken").unwrap();
        assert!(standalone_default().is_err());
    }
    #[test]
    fn requests_validate_modes_and_do_not_guess_from_version_strings() {
        for (mode, emulator) in [
            (ExecutionMode::Intel, Emulator::Rosetta),
            (ExecutionMode::Arm64Fex, Emulator::Fex),
        ] {
            let (_temp, runtime) = crate::runtime::fixture(mode);
            assert_eq!(
                resolve(runtime.root.to_str(), Some(emulator))
                    .unwrap()
                    .unwrap()
                    .root,
                runtime.root
            );
            let other = if emulator == Emulator::Fex {
                Emulator::Rosetta
            } else {
                Emulator::Fex
            };
            assert!(resolve(runtime.root.to_str(), Some(other)).is_err());
        }
        assert!(resolve(None, None).unwrap().is_none());
        assert!(named("/missing/fex-preview").is_err());
        assert!("box64".parse::<Emulator>().is_err());
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    schema: u32,
    root: std::path::PathBuf,
    #[serde(default)]
    managed: bool,
    prefix: crate::runtime::PrefixCompatibility,
}

fn load_fex_selection(path: &Path) -> Result<ResolvedRuntime> {
    let selection: Selection = serde_json::from_slice(
        &std::fs::read(path)
            .with_context(|| format!("Read experimental runtime selection {}", path.display()))?,
    )
    .with_context(|| format!("Invalid experimental runtime selection {}", path.display()))?;
    ensure!(
        selection.schema == 1,
        "Unsupported experimental tool selection schema"
    );
    ensure!(
        !selection.managed
            || selection
                .root
                .join(crate::managed_runtime::RECEIPT)
                .is_file(),
        "Managed runtime installation is incomplete. Run 'nucleon wine fex verify' or reinstall the verified source"
    );
    let runtime = ResolvedRuntime::inspect(&selection.root)?;
    ensure!(
        runtime.mode == ExecutionMode::Arm64Fex
            && runtime.root == selection.root
            && runtime.prefix == selection.prefix,
        "Native runtime identity changed. Select it again with 'nucleon wine fex select <runtime>'"
    );
    Ok(runtime)
}
