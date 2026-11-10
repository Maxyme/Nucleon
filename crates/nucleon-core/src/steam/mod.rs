//! Steam metadata and Nucleon-managed game installations.
mod appinfo;
pub mod library;
use crate::paths;
use anyhow::{Context, Result, ensure};
use std::path::PathBuf;

pub fn check_steam_installed() -> Result<PathBuf> {
    let app = paths::steam_app();
    ensure!(app.is_dir(), "Steam.app not found at {}", app.display());
    Ok(app)
}

pub fn is_steam_running() -> bool {
    running().unwrap_or(false)
}

pub fn running() -> Result<bool> {
    use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};
    let mut system = System::new().context("Cannot inspect Steam processes")?;
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    Ok(system
        .processes()
        .values()
        .any(|process| process.status() != ProcessStatus::Zombie && process.name() == "steam_osx"))
}
