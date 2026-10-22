use anyhow::{Result, ensure};
use nucleon_core::{guard, paths, steam, validator};
use std::process::Command;

pub fn run_launch(
    appid: u32,
    hud: bool,
    engine: Option<String>,
    wine: Option<String>,
    emulator: Option<nucleon_core::runtime_selection::Emulator>,
    upscaler: Option<nucleon_core::upscaler::Backend>,
) -> Result<()> {
    let mut request = prepare_request(hud, engine.as_deref(), wine.as_deref(), emulator)?;
    request.upscaler = upscaler.map(nucleon_core::upscaler::Settings::selected);
    if let Some(path) = &request.wine {
        println!("  Wine: {path}");
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
    paths::ensure_dirs()?;
    nucleon_core::fs_util::atomic_write_file(&override_file, serde_json::to_vec(&request)?)?;

    let mut cmd = Command::new("open");
    cmd.arg(format!("steam://run/{}", appid));
    ensure!(cmd.status()?.success(), "Steam launch request failed");
    println!("  ✓ Sent launch command to Steam");

    Ok(())
}

fn prepare_request(
    hud: bool,
    engine: Option<&str>,
    wine: Option<&str>,
    emulator: Option<nucleon_core::runtime_selection::Emulator>,
) -> Result<nucleon_core::profile::GameProfile> {
    let selected_engine = engine
        .map(|value| nucleon_core::configuration::engine("--engine", value))
        .transpose()?;
    let runtime = nucleon_core::runtime_selection::resolve(wine, emulator)?;
    if let Some(runtime) = &runtime {
        nucleon_core::runtime_selection::check_host(runtime)?;
        ensure!(
            selected_engine != Some(nucleon_core::detector::TargetEngine::Gptk),
            "An explicit Wine selection cannot use the separate GPTK runner; choose auto, dxmt, dxvk, vkd3d, kosmickrisp or wine"
        );
    }
    Ok(nucleon_core::profile::GameProfile {
        hud: Some(hud),
        engine: engine
            .map(str::to_owned)
            .or_else(|| runtime.as_ref().map(|_| "auto".into())),
        wine: runtime.map(|r| r.root.to_string_lossy().into_owned()),
        emulator,
        ..Default::default()
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_keep_explicit_auto_and_reject_invalid_selectors_before_side_effects() {
        let request = prepare_request(false, Some("auto"), None, None).unwrap();
        assert_eq!(request.engine.as_deref(), Some("auto"));
        assert_eq!(request.hud, Some(false));
        assert!(request.wine.is_none());
        assert!(prepare_request(false, Some("typo"), None, None).is_err());
        let error =
            prepare_request(false, None, Some("/missing/requested-runtime"), None).unwrap_err();
        assert!(error.to_string().contains("/missing/requested-runtime"));
    }
}
