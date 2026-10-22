use super::{PreparedLaunch, RunSession, config, execute_launch, finish_run, log_runner};
use anyhow::{Context, Result};
use nucleon_core::{
    application::{Application, Bottle, Settings},
    detector::TargetEngine,
    launch::{GraphicsComponents, LaunchPlan},
    paths,
};
use std::path::{Path, PathBuf};

pub struct StandaloneRequest {
    pub application: Application,
    pub bottle: Option<String>,
    pub settings: Settings,
    pub prefix: Option<PathBuf>,
}

pub fn run_standalone(request: StandaloneRequest) -> Result<()> {
    anyhow::ensure!(
        request.prefix.is_none() || request.bottle.is_none(),
        "Choose --prefix or --bottle, not both"
    );
    let external_prefix = request
        .prefix
        .as_deref()
        .map(nucleon_core::application::working_directory)
        .transpose()?;
    let saved_upscaler = request.application.upscaler.clone();
    let mut settings = request.settings;
    settings.upscaler = settings.upscaler.or(saved_upscaler);
    let application = Application::new(
        &request.application.executable,
        request.application.arguments,
        request.application.cwd.as_deref(),
    )?;
    let (identity, data_path, settings) = if let Some(prefix) = &external_prefix {
        let (identity, _) = nucleon_core::application::standalone_data(prefix)?;
        (
            format!("prefix-{identity}"),
            prefix.join(".nucleon-data"),
            settings,
        )
    } else if let Some(name) = &request.bottle {
        let bottle = Bottle::load(name)?;
        (
            format!("bottle-{name}"),
            bottle.data_directory()?,
            settings.with_defaults(bottle.settings),
        )
    } else {
        let (identity, data) = nucleon_core::application::standalone_data(&application.executable)?;
        (identity, data, settings)
    };
    let mut arguments = vec![
        application
            .executable
            .to_str()
            .context("Executable path must be valid UTF-8")?
            .to_owned(),
    ];
    arguments.extend(application.arguments);
    let mut receipt_args = vec!["nucleon".into(), "run".into()];
    receipt_args.extend(arguments.clone());
    let mut session = RunSession::standalone(&receipt_args, &identity)?;
    log_runner(&format!("Launch logs: {}", session.directory.display()));
    let result = session.configure().and_then(|()| {
        let mut plan = prepare(&application.executable, &data_path, &settings)?;
        if let Some(prefix) = external_prefix {
            anyhow::ensure!(plan.runtime.mode == nucleon_core::runtime::ExecutionMode::Intel,
                "Existing external prefixes require Intel/Rosetta Wine; use a separate managed prefix for FEX");
            plan.prefix = prefix;
        }
        execute_launch(
            PreparedLaunch {
                plan,
                data_path,
                appid: None,
                arguments,
                cwd: application.cwd,
                hud: settings.hud.unwrap_or(false),
                overlay: false,
                wait: false,
                consume_override: None,
                upscaler: settings.upscaler.clone(),
            },
            &mut session,
        )
    });
    finish_run(&mut session, result)
}

fn prepare(executable: &Path, data: &Path, settings: &Settings) -> Result<LaunchPlan> {
    settings.validate()?;
    let guest = nucleon_core::detector::executable_guest(executable)?;
    let runtime = settings.resolve_runtime()?;
    runtime.require_guest(guest == nucleon_core::runtime::GuestAbi::X64)?;
    nucleon_core::runtime_selection::check_host(&runtime)?;
    let detection = nucleon_core::detector::detect_target_engine(executable);
    let api = settings
        .profile()
        .graphics_api(executable, &detection.api)?;
    let mut components = GraphicsComponents::discover(&runtime, None, None)?;
    components.gptk = GraphicsComponents::bundled_gptk(&runtime);
    let engine = match settings
        .engine
        .as_deref()
        .map(|v| config::engine("--engine", v))
        .transpose()?
    {
        None | Some(TargetEngine::Auto) => {
            let available = components.available(
                &runtime,
                &api,
                guest == nucleon_core::runtime::GuestAbi::X64,
            );
            available.select(&api)?
        }
        Some(engine) => engine,
    };
    anyhow::ensure!(
        engine != TargetEngine::Gptk
            || (components.gptk && guest == nucleon_core::runtime::GuestAbi::X64),
        "GPTK requires a complete compatible Intel Wine runtime with bundled D3DMetal and an x64 target; select it with --wine, or choose another engine"
    );
    log_runner(&format!(
        "Standalone target '{}' [{api:?}] -> {}",
        executable.display(),
        engine.display_name()
    ));
    let prefix = paths::game_prefix(data, &runtime);
    Ok(LaunchPlan::new(
        runtime,
        prefix,
        engine,
        components,
        guest == nucleon_core::runtime::GuestAbi::X64,
        Some(false),
        &api,
    )?
    .without_steam())
}

/// Unlike game classification, explicit tools and installer helpers remain live applications.
pub(super) fn is_application_process_line(
    line: &str,
    my_pid: u32,
    _runner: &str,
    target: &str,
) -> bool {
    let mut fields = line.trim().splitn(2, char::is_whitespace);
    let pid = fields.next().unwrap_or("");
    if pid.parse::<u32>().ok() == Some(my_pid) {
        return false;
    }
    let command = fields.next().unwrap_or("").trim().to_lowercase();
    if command.is_empty() || command.contains("nucleon-runner") || command.contains("/nucleon ") {
        return false;
    }
    if !target.is_empty() && command.contains(target) {
        return true;
    }
    if [
        "wineserver",
        "winedevice.exe",
        "services.exe",
        "plugplay.exe",
        "svchost.exe",
        "rpcss.exe",
        "explorer.exe",
        "conhost.exe",
        "wineboot.exe",
        "winemenubuilder.exe",
        "crashpad_handler",
        "unitycrashhandler",
    ]
    .iter()
    .any(|name| command.contains(name))
    {
        return false;
    }
    // The caller has already checked this process's exact WINEPREFIX. Children
    // can rename themselves and need not retain Wine's loader in their command.
    command.contains(".exe")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tools_and_installer_handoffs_are_live_but_services_are_not() {
        for exe in [
            "msiexec.exe",
            "rundll32.exe",
            "reg.exe",
            "winecfg.exe",
            "game.exe",
        ] {
            let line = format!("123 /runtime/bin/wine {exe}");
            assert!(
                is_application_process_line(&line, 10, "/runtime", "setup.exe"),
                "{line}"
            );
        }
        assert!(is_application_process_line(
            "123 /runtime/bin/wine explorer.exe",
            10,
            "/runtime",
            "explorer.exe"
        ));
        for line in [
            "123 /runtime/bin/wineserver",
            "123 /runtime/bin/wine services.exe",
            "10 /runtime/bin/wine setup.exe",
            "123 /bin/sh unrelated",
        ] {
            assert!(
                !is_application_process_line(line, 10, "/runtime", "setup.exe"),
                "{line}"
            );
        }
    }
}
