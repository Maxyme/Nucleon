use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::PathBuf;

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn support_dir() -> PathBuf {
    if let Ok(dir) = env::var("NUCLEON_SUPPORT_DIR") {
        return PathBuf::from(dir);
    }
    home_dir().join("Library/Application Support/nucleon")
}

/// Latest launch pointer; historical runs stay in AppID/run-specific directories.
pub fn latest_run_dir() -> Result<Option<PathBuf>> {
    let logs = support_dir().join("logs");
    let Some(text) = crate::configuration::optional_file(&logs.join("latest-run.json"))? else {
        return Ok(None);
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Latest {
        directory: PathBuf,
    }
    let latest: Latest = serde_json::from_str(&text).context("Invalid latest-run log pointer")?;
    anyhow::ensure!(
        !latest.directory.as_os_str().is_empty()
            && latest
                .directory
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "Invalid latest-run log path"
    );
    Ok(Some(logs.join(latest.directory)))
}

pub fn runners_dir() -> PathBuf {
    support_dir().join("runners")
}

pub fn current_runner() -> PathBuf {
    runners_dir().join("current")
}

pub fn bridge_dir() -> PathBuf {
    support_dir().join("bridge")
}

pub fn downloads_dir() -> PathBuf {
    support_dir().join("downloads")
}

pub fn steam_app() -> PathBuf {
    PathBuf::from("/Applications/Steam.app")
}

pub fn steam_data_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Steam")
}

pub fn game_profiles_dir(fex_tool: bool) -> PathBuf {
    let base = support_dir().join("game_profiles");
    if fex_tool {
        base.join("arm64-fex")
    } else {
        base
    }
}

pub fn game_prefix(
    compat_data: &std::path::Path,
    runtime: &crate::runtime::ResolvedRuntime,
) -> PathBuf {
    match runtime.mode {
        crate::runtime::ExecutionMode::Intel => compat_data.join("pfx"),
        crate::runtime::ExecutionMode::Arm64Fex => compat_data
            .join("nucleon/arm64-fex")
            .join(&runtime.prefix.family)
            .join("pfx"),
    }
}

/// Existing prefix locations for diagnostics, including native runtime families.
pub fn game_prefixes(compat_data: &std::path::Path) -> Vec<PathBuf> {
    let mut prefixes = vec![compat_data.join("pfx")];
    if let Ok(entries) = fs::read_dir(compat_data.join("nucleon/arm64-fex")) {
        let mut native = entries
            .flatten()
            .map(|e| e.path().join("pfx"))
            .filter(|p| p.is_dir() && !p.is_symlink())
            .collect::<Vec<_>>();
        native.sort();
        prefixes.extend(native);
    }
    prefixes
}

pub fn kosmickrisp_dir() -> PathBuf {
    support_dir().join("drivers/kosmickrisp")
}

pub fn kosmickrisp_shim_dir() -> PathBuf {
    support_dir().join("shims/kosmickrisp")
}

pub fn vkd3d_proton_dir() -> PathBuf {
    support_dir().join("vkd3d-proton")
}

pub fn d7vk_dir() -> PathBuf {
    support_dir().join("d7vk")
}

pub fn dxvk_dir() -> PathBuf {
    support_dir().join("dxvk")
}

pub fn dxmt_dir() -> PathBuf {
    support_dir().join("dxmt")
}

pub fn ensure_dirs() -> Result<()> {
    for d in &[
        support_dir(),
        support_dir().join("bin"),
        runners_dir(),
        bridge_dir(),
        downloads_dir(),
        kosmickrisp_dir(),
        kosmickrisp_shim_dir(),
        vkd3d_proton_dir(),
        d7vk_dir(),
        dxvk_dir(),
        dxmt_dir(),
    ] {
        fs::create_dir_all(d)
            .with_context(|| format!("Failed to create directory: {}", d.display()))?;
    }
    Ok(())
}
