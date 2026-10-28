use anyhow::{Result, ensure};
use nucleon_core::{paths, steam, validator};

pub fn run_launch(args: super::steam::PlayArgs) -> Result<()> {
    let installed = steam::library::installed(args.appid)?;
    let mut target =
        steam::library::target(&installed, args.exe.as_deref(), args.launch_id.as_deref())?;
    target.arguments.extend(args.arguments);
    let mut overrides = prepare_request(
        args.hud,
        args.engine.as_deref(),
        args.wine.as_deref(),
        args.emulator,
    )?;
    overrides.upscaler = args
        .upscaler
        .map(nucleon_core::upscaler::Settings::selected);
    nucleon_runner::launch_steam(
        nucleon_runner::SteamRequest {
            appid: args.appid,
            executable: target.executable,
            arguments: target.arguments,
            cwd: Some(target.cwd),
            data_path: installed.data_path,
            overrides,
        },
        args.dry_run,
    )
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
    let compat_data = steam::library::installed(appid)?.data_path;
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
