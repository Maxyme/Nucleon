use super::{PreparedLaunch, ResolvedLaunch, RunSession, execute_launch, finish_run, log_runner};
use anyhow::{Context, Result};
use nucleon_core::application::{Application, Bottle, Settings};
use std::path::PathBuf;

pub struct StandaloneRequest {
    pub application: Application,
    pub bottle: Option<String>,
    pub settings: Settings,
    pub prefix: Option<PathBuf>,
}

/// A concrete launch, resolved without creating prefixes, logs, or obtaining credentials.
/// Execution consumes this value, so it cannot silently select another runtime.
pub struct ResolvedStandalone {
    launch: ResolvedLaunch,
    identity: String,
}

pub fn run_standalone(request: StandaloneRequest) -> Result<()> {
    resolve_standalone(request)?.run(Default::default())
}

pub fn resolve_standalone(request: StandaloneRequest) -> Result<ResolvedStandalone> {
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
    let plan = nucleon_core::application::resolve_launch(
        &application.executable,
        &data_path,
        external_prefix.as_deref(),
        &settings,
    )?;
    if external_prefix.is_some() {
        anyhow::ensure!(
            plan.runtime.mode == nucleon_core::runtime::ExecutionMode::Intel,
            "Existing external prefixes require Intel/Rosetta Wine; use a separate managed prefix for FEX"
        );
    }
    let launch = PreparedLaunch {
        plan,
        data_path,
        appid: None,
        arguments,
        cwd: application.cwd,
        hud: settings.hud.unwrap_or(false),
        overlay: false,
        upscaler: settings.upscaler,
        environment: Default::default(),
    }
    .resolve()?;
    Ok(ResolvedStandalone { launch, identity })
}

impl ResolvedStandalone {
    /// Only argument values are redacted; runtime and graphics choices are the exact
    /// values subsequently consumed by execution.
    pub fn preview(&self) -> serde_json::Value {
        let p = &self.launch.prepared;
        serde_json::json!({
            "executable": p.arguments[0], "prefix": p.plan.prefix, "cwd": p.cwd,
            "runtime": p.plan.runtime.root, "engine": p.plan.engine.display_name(),
            "graphics_api": format!("{:?}", p.plan.api), "is_64_bit": p.plan.is64, "hud": p.hud,
            "graphics": {
                "dxmt": p.plan.graphics.dxmt.as_ref().map(|c| &c.root),
                "dxvk": p.plan.graphics.dxvk.as_ref().map(|c| &c.root),
                "vkd3d": p.plan.graphics.vkd3d.as_ref().map(|c| &c.root),
                "d7vk": p.plan.graphics.d7vk.as_ref().map(|c| &c.root),
                "vulkan": p.plan.graphics.vulkan.iter().map(|c| serde_json::json!({
                    "manifest": c.manifest, "library": c.library,
                })).collect::<Vec<_>>(),
                "bundled_gptk": p.plan.graphics.gptk,
            },
            "arguments": nucleon_core::launch_profile::redact_arguments(&p.arguments[1..]),
            "upscaler": self.launch.resolved_scaler,
            "authenticated": false, "executed": false,
        })
    }

    /// Store credentials are obtained only after successful resolution, and are
    /// appended without changing the already selected runtime or graphics plan.
    pub fn append_arguments(&mut self, arguments: impl IntoIterator<Item = String>) {
        self.launch.prepared.arguments.extend(arguments);
    }

    pub fn run(mut self, environment: std::collections::BTreeMap<String, String>) -> Result<()> {
        self.launch.prepared.environment = environment;
        let mut receipt_args = vec!["nucleon".into(), "run".into()];
        receipt_args.extend(self.launch.prepared.arguments.clone());
        let mut session = RunSession::standalone(&receipt_args, &self.identity)?;
        log_runner(&format!("Launch logs: {}", session.directory.display()));
        let result = session
            .configure()
            .and_then(|()| execute_launch(self.launch, &mut session));
        finish_run(&mut session, result)
    }
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
    if command.is_empty() || command.contains("/nucleon ") {
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
