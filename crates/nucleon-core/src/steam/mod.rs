//! Read-only Steam discovery and one-time removal of legacy client modifications.
mod appinfo;
pub mod library;
pub mod migration;
pub mod native_detect;
mod vdf_mapping;
use crate::paths;
use anyhow::{Context, Result, bail, ensure};
pub use native_detect::*;
use std::{path::PathBuf, process::Command};

pub fn check_steam_installed() -> Result<PathBuf> {
    let app = paths::steam_app();
    ensure!(app.is_dir(), "Steam.app not found at {}", app.display());
    Ok(app)
}

pub fn is_steam_running() -> bool {
    running().unwrap_or(false)
}

pub fn running() -> Result<bool> {
    let output = Command::new("pgrep")
        .args(["-x", "steam_osx"])
        .output()
        .context("Check whether Steam is running")?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "Cannot inspect Steam processes: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}
