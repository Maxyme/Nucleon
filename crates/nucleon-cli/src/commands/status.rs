use super::ui;
use anyhow::Result;
use nucleon_core::{paths, runner, steam, wine};

pub fn run() -> Result<()> {
    ui::header("Nucleon Status Report");
    ui::kv(
        "Steam launch mode:",
        "Direct through Nucleon; no client patches or update guard",
    );
    ui::status_bool("Steam installed:", steam::check_steam_installed().is_ok());
    ui::status_bool("Steam running:", steam::is_steam_running());
    if steam::migration::present() {
        ui::warn(
            "Legacy Steam integration is still installed. Quit Steam and run 'nucleon steam detach'.",
        );
    }
    if let Some(runtime) = wine::get_active_wine_runtime() {
        ui::kv("Intel Wine:", runtime.root.display());
    }
    if paths::current_runner().exists() {
        ui::kv("GPTK runtime:", paths::current_runner().display());
    }
    if let Ok(runtime) = nucleon_core::runtime_selection::fex_default() {
        ui::kv("FEX runtime:", runtime.root.display());
    }
    let (backend, origin) = nucleon_core::backend::get_active_backend_with_origin();
    ui::kv(
        "Graphics selection:",
        format!("{} ({origin})", backend.as_str()),
    );
    if let Some((components, origin)) = runner::get_gptk_resolution_info(None) {
        ui::kv(
            "Apple D3DMetal:",
            format!("{} ({origin})", components.0.display()),
        );
    }
    ui::status_bool(
        "Steamworks bridge staged:",
        paths::bridge_dir().join("steamclient64.dll").is_file(),
    );
    ui::kv(
        "Download manager:",
        "DepotDownloader (nucleon steam install/update)",
    );
    Ok(())
}
