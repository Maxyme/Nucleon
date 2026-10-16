use anyhow::Result;
use nucleon_core::{guard, paths, steam, validator};
use std::process::Command;

pub fn run_launch(appid: u32, hud: bool, engine: Option<String>) -> Result<()> {
    if let Some(value) = engine.as_deref() {
        nucleon_core::configuration::engine("--engine", value)?;
    }
    println!("==> Launching game AppID {}...", appid);

    // Ensure Steam update guard, permissions, manifests, WebUI, and compat mappings are clean
    let _ = guard::run_guard_once(super::find_hook_dylib().as_deref());
    let _ = steam::fix_steam_permissions();
    let _ = steam::sanitize_installed_app_manifests();
    let _ = steam::patch_steamui_chunks();
    let _ = steam::migrate_compat_mappings();

    // Persist launch overrides so nucleon-runner reads them even with running Steam
    let override_file = paths::support_dir().join(format!("launch_override_{appid}.json"));
    let mut ov = serde_json::json!({
        "hud": hud,
    });
    if let Some(ref eng) = engine {
        ov["engine"] = serde_json::Value::String(eng.clone());
    }
    paths::ensure_dirs()?;
    nucleon_core::fs_util::atomic_write_file(&override_file, ov.to_string())?;

    let mut cmd = Command::new("open");
    cmd.arg(format!("steam://run/{}", appid));
    if hud {
        cmd.env("MTL_HUD_ENABLED", "1");
    }
    if let Some(eng) = engine {
        cmd.env("NUCLEON_ENGINE", eng);
    }
    cmd.status()?;
    println!("  ✓ Sent launch command to Steam");

    Ok(())
}

pub fn run_validate(appid: u32) -> Result<()> {
    println!(
        "==> Validating window presentation for AppID {} (zero screen capture)...",
        appid
    );
    let compat_data = paths::steam_compat_data_dir().join(appid.to_string());
    let pids = paths::game_prefixes(&compat_data)
        .into_iter()
        .flat_map(|prefix| nucleon_runner::wine_process_pids(&prefix))
        .collect::<Vec<_>>();
    let presented = validator::check_window_presentation(Some(&pids))?;
    if presented.is_empty() {
        println!("  ! No active Wine window currently presenting on screen.");
    } else {
        for w in presented {
            println!(
                "  ✓ Window ID: {} | PID: {} | Owner: {} | Size: {}x{} | Layer: {} | OnScreen: {}",
                w.window_id, w.pid, w.owner_name, w.width, w.height, w.layer, w.is_onscreen
            );
        }
    }

    Ok(())
}
