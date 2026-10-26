#![forbid(unsafe_code)]

use anyhow::{Context, Result, bail};
use nucleon_core::configuration as config;
use nucleon_core::launch::{GraphicsComponents, LaunchPlan};
use nucleon_core::prefix_session::PrefixSession;
use nucleon_core::profile::GameProfile as LaunchOverride;
use nucleon_core::runtime::ResolvedRuntime;
use nucleon_core::{paths, runner};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use sysinfo::{ProcessesToUpdate, System};

mod process;
mod session;
mod standalone;
use process::{OwnedChild, TERMINATION_GRACE};
use session::{RunSession, trace as trace_runner};
pub use standalone::{ResolvedStandalone, StandaloneRequest, resolve_standalone, run_standalone};

/// Resolve once so compatibility checks and execution use the same Wine.
fn resolve_wine_override(
    game_profile: &LaunchOverride,
    profile_source: &str,
) -> Result<Option<PathBuf>> {
    if let Some(value) = &game_profile.wine {
        let runtime = nucleon_core::runtime_selection::named(value).with_context(|| format!("Invalid {profile_source}: wine={value:?}; run 'nucleon wine list' and choose an exact installed ID or path"))?;
        return Ok(Some(runtime.root));
    }
    // A stale saved global selection must not silently select a different runtime.
    let saved = nucleon_core::wine::active_wine_selection_file();
    if let Some(value) = config::optional_file(&saved)? {
        let value = value.trim();
        if value.is_empty() || nucleon_core::wine::resolve_wine_runtime_by_query(value).is_none() {
            return Err(config::invalid(
                &saved.display().to_string(),
                value,
                "saved Wine runtime was not found",
                "Run 'nucleon wine list', then 'nucleon wine use <name>', or 'nucleon wine reset'.",
            ));
        }
        return Ok(nucleon_core::wine::resolve_wine_runtime_by_query(value).map(|r| r.root));
    }
    config::component_path(
        &[],
        &nucleon_core::wine::custom_wine_path_file(),
        "Wine installation was not found",
        "nucleon wine clear-path",
        |path| nucleon_core::wine::inspect_wine_dir(path, None, None).is_some(),
    )?;
    Ok(None)
}

fn load_launch_settings(path: &Path) -> Result<Option<LaunchOverride>> {
    LaunchOverride::load(path)
}

fn requested_engine(
    temporary: Option<&LaunchOverride>,
    temporary_source: &str,
    environment: Option<&str>,
    profile: &LaunchOverride,
    profile_source: &str,
) -> Result<Option<nucleon_core::detector::TargetEngine>> {
    if let Some(value) = temporary.and_then(|p| p.engine.as_deref()) {
        return config::engine(&format!("{temporary_source}: engine"), value).map(Some);
    }
    if let Some(value) = environment {
        return config::engine("NUCLEON_ENGINE", value).map(Some);
    }
    profile
        .engine
        .as_deref()
        .map(|value| config::engine(&format!("{profile_source}: engine"), value))
        .transpose()
}

fn environment_boolean(name: &str) -> Result<Option<bool>> {
    config::environment(name)?
        .map(|v| config::boolean(name, &v))
        .transpose()
}

fn requested_upscaler(
    temporary: Option<&LaunchOverride>,
    environment: Option<&str>,
    profile: &LaunchOverride,
) -> Result<Option<nucleon_core::upscaler::Settings>> {
    if let Some(settings) = temporary.and_then(|p| p.upscaler.clone()) {
        return Ok(Some(settings));
    }
    if let Some(value) = environment {
        return Ok(Some(nucleon_core::upscaler::Settings::selected(
            value.parse()?,
        )));
    }
    Ok(profile.upscaler.clone())
}

fn validate_graphics_paths(
    profile: &LaunchOverride,
    source: &str,
    runtime: Option<&ResolvedRuntime>,
    engine: Option<nucleon_core::detector::TargetEngine>,
) -> Result<()> {
    use nucleon_core::detector::TargetEngine;
    let automatic = engine.is_none_or(|e| e == TargetEngine::Auto);
    let uses_dxmt = automatic || engine == Some(TargetEngine::Dxmt);
    let uses_dxvk = automatic
        || matches!(
            engine,
            Some(TargetEngine::Dxvk | TargetEngine::Vkd3d | TargetEngine::KosmicKrisp)
        );
    let uses_vkd3d = automatic
        || matches!(
            engine,
            Some(TargetEngine::Vkd3d | TargetEngine::KosmicKrisp)
        );
    let uses_d7vk = automatic || engine == Some(TargetEngine::KosmicKrisp);
    let uses_vulkan = uses_dxvk || uses_vkd3d || uses_d7vk;
    let bundled = runtime.map(|r| &r.graphics);
    use nucleon_core::{d7vk, dxmt, dxvk, vkd3d};
    if config::environment("DXVK_PATH")?.is_some()
        || (uses_dxvk
            && profile.dxvk_path.is_none()
            && bundled.and_then(|b| b.dxvk.as_ref()).is_none())
    {
        config::component_path(
            &["DXVK_PATH"],
            &dxvk::custom_dxvk_path_file(),
            "DXVK bundle was not found (expected x64/x32 graphics DLLs)",
            "nucleon dxvk clear-path",
            |p| dxvk::inspect_candidate_dir(p).is_some(),
        )?;
    }
    if (uses_dxmt && bundled.and_then(|b| b.dxmt.as_ref()).is_none())
        || env::var_os("DXMT_PATH").is_some()
    {
        config::component_path(
            &["DXMT_PATH"],
            &dxmt::custom_dxmt_path_file(),
            "DXMT bundle was not found (expected d3d11.dll and Metal components)",
            "nucleon dxmt clear-path",
            |p| dxmt::inspect_candidate_dir(p).is_some(),
        )?;
    }
    if (uses_vkd3d && bundled.and_then(|b| b.vkd3d.as_ref()).is_none())
        || ["VKD3D_PROTON_PATH", "VKD3D_PATH"]
            .iter()
            .any(|n| env::var_os(n).is_some())
    {
        config::component_path(
            &["VKD3D_PROTON_PATH", "VKD3D_PATH"],
            &vkd3d::custom_vkd3d_path_file(),
            "VKD3D-Proton bundle was not found (expected d3d12.dll)",
            "nucleon vkd3d clear-path",
            |p| vkd3d::inspect_candidate_dir(p).is_some(),
        )?;
    }
    if uses_d7vk || env::var_os("D7VK_PATH").is_some() {
        config::component_path(
            &["D7VK_PATH"],
            &d7vk::custom_d7vk_path_file(),
            "D7VK bundle was not found (expected ddraw.dll)",
            "nucleon d7vk clear-path",
            |p| d7vk::inspect_candidate_dir(p).is_some(),
        )?;
    }
    if config::environment("DXVK_PATH")?.is_none()
        && let Some(path) = &profile.dxvk_path
        && dxvk::inspect_candidate_dir(path).is_none()
    {
        return Err(config::invalid(
            &format!("{source}: dxvk_path"),
            &path.display().to_string(),
            "DXVK bundle was not found",
            "Correct dxvk_path or remove that field to use the default bundle.",
        ));
    }
    let mut driver_override = false;
    for name in ["VK_DRIVER_FILES", "VK_ICD_FILENAMES"] {
        if let Some(value) = config::environment(name)? {
            let paths = env::split_paths(&value).collect::<Vec<_>>();
            if paths.is_empty() || paths.iter().any(|p| p.as_os_str().is_empty()) {
                return Err(config::invalid(
                    name,
                    &value,
                    "expected ICD JSON paths",
                    "Use a manifest path (or colon-separated list), or remove the override.",
                ));
            }
            for path in paths {
                config::vulkan_manifest(name, &path)?;
            }
            driver_override = true;
            break;
        }
    }
    if !driver_override {
        if let Some(path) = &profile.vulkan_icd {
            config::vulkan_manifest(&format!("{source}: vulkan_icd"), path)?;
        } else if let Some(value) = config::environment("KOSMICKRISP_ICD_PATH")? {
            config::vulkan_manifest("KOSMICKRISP_ICD_PATH", Path::new(&value))?;
            driver_override = true;
        }
    }
    if uses_vulkan
        && !driver_override
        && profile.vulkan_icd.is_none()
        && bundled.and_then(|b| b.vulkan_icd.as_ref()).is_none()
    {
        let saved = runner::custom_kosmickrisp_path_file();
        if let Some(value) = config::optional_file(&saved)? {
            let info = runner::inspect_kosmickrisp_candidate(Path::new(value.trim()), None)
                .with_context(|| format!("Invalid Vulkan driver path in {}. Correct it or run 'nucleon kosmickrisp clear-path'", saved.display()))?;
            if !info.library_path.is_file() {
                bail!(
                    "Invalid Vulkan driver path in {}: library {} is missing. Correct the path or run 'nucleon kosmickrisp clear-path'",
                    saved.display(),
                    info.library_path.display()
                );
            }
        }
    }
    Ok(())
}

/// Explicit backend choices win; inferred GPTK must not discard a per-game Wine selection.
fn select_automatic_engine(
    api: &nucleon_core::detector::GraphicsApi,
    mut available: nucleon_core::detector::AutoBackends,
    configured: nucleon_core::backend::GraphicsBackend,
    custom_wine: bool,
) -> Result<nucleon_core::detector::TargetEngine> {
    use nucleon_core::backend::GraphicsBackend;
    use nucleon_core::detector::TargetEngine;
    available.gptk &= !custom_wine;
    match configured {
        GraphicsBackend::Auto => available.select(api),
        GraphicsBackend::D3DMetal => Ok(TargetEngine::Gptk),
        GraphicsBackend::Dxmt => Ok(TargetEngine::Dxmt),
        GraphicsBackend::Dxvk => Ok(TargetEngine::Dxvk),
        GraphicsBackend::KosmicKrisp => Ok(TargetEngine::KosmicKrisp),
    }
}

/// Determines whether to launch the game through the prefix's `steam.exe` client shim or directly.
///
/// Steamworks imports do not determine the launch protocol: 64-bit games such as DiRT Rally 2.0
/// must launch directly even though they talk to Steam. The default preserves the existing legacy
/// wrapper for 32-bit titles and direct execution for 64-bit titles, independently of graphics.
/// Explicit user overrides (via CLI `use_steam_shim` or `NUCLEON_STEAM_SHIM` env var) take precedence.
pub fn determine_wine_args(
    game_args: &[String],
    default_use_shim: bool,
    steam_shim_exists: bool,
    user_shim_override: Option<bool>,
) -> Vec<String> {
    let use_shim = match user_shim_override {
        Some(explicit) => explicit && steam_shim_exists,
        None => default_use_shim && steam_shim_exists,
    };

    if use_shim {
        let mut wrapped = vec!["C:\\Program Files (x86)\\Steam\\steam.exe".to_string()];
        wrapped.extend(game_args.to_vec());
        wrapped
    } else {
        game_args.to_vec()
    }
}

fn log_runner(msg: &str) {
    session::log(msg);
}

fn set_runner_lib_env(cmd: &mut Command, runner_dir: &Path) {
    let lib_dir = runner_dir.join("lib");
    let lib_unix = ResolvedRuntime::inspect(runner_dir)
        .map(|r| r.unix_dir)
        .unwrap_or_else(|_| runner_dir.join("lib/wine/x86_64-unix"));
    if lib_dir.is_dir() || lib_unix.is_dir() {
        cmd.env(
            "DYLD_FALLBACK_LIBRARY_PATH",
            format!("{}:{}", lib_unix.display(), lib_dir.display()),
        );
    }
}

/// Always reap a helper after it exits or is killed at its deadline.
fn wait_child_with_timeout(
    child: &mut OwnedChild,
    timeout: Duration,
) -> Result<Option<std::process::ExitStatus>> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                child.terminate(TERMINATION_GRACE)?;
                return Ok(Some(status));
            }
            Ok(None) if start.elapsed() < timeout => {
                thread::sleep(
                    Duration::from_millis(50).min(timeout.saturating_sub(start.elapsed())),
                );
            }
            result => {
                child
                    .terminate(TERMINATION_GRACE)
                    .context("Failed to stop subprocess helper group")?;
                if let Err(error) = result {
                    return Err(error).context("Failed to poll subprocess helper");
                }
                return Ok(None);
            }
        }
    }
}

fn run_wineserver_helper(
    wineserver: &Path,
    prefix: &Path,
    argument: &str,
    timeout: Duration,
    runtime: Option<&ResolvedRuntime>,
) -> Result<()> {
    if !wineserver.is_file() {
        return Ok(());
    }
    let mut command = runtime
        .map(|r| r.command(wineserver))
        .unwrap_or_else(|| Command::new(wineserver));
    nucleon_core::launch::clean_command(&mut command);
    command.arg(argument).env("WINEPREFIX", prefix);
    if let Some(runtime) = runtime {
        command
            .env("DYLD_LIBRARY_PATH", &runtime.unix_dir)
            .env("DYLD_FALLBACK_LIBRARY_PATH", runtime.root.join("lib"));
    } else if let Some(runner) = wineserver.parent().and_then(Path::parent) {
        set_runner_lib_env(&mut command, runner);
    }
    let mut child = OwnedChild::spawn(&mut command).with_context(|| {
        format!(
            "Cannot start wineserver {argument} at {}",
            wineserver.display()
        )
    })?;
    wait_child_with_timeout(&mut child, timeout)?.with_context(|| {
        format!(
            "Wineserver {argument} helper timed out after {} ms",
            timeout.as_millis()
        )
    })?;
    Ok(())
}

fn wait_wineserver(runtime: &ResolvedRuntime, prefix: &Path, timeout_ms: u64) {
    if let Err(error) = run_wineserver_helper(
        &runtime.server,
        prefix,
        "-w",
        Duration::from_millis(timeout_ms),
        Some(runtime),
    ) {
        log_runner(&format!("Wineserver wait failed: {error:#}"));
    }
}

fn termination_flag() -> Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    let handler_flag = Arc::clone(&flag);
    ctrlc::set_handler(move || handler_flag.store(true, Ordering::SeqCst))
        .context("Cannot install SIGINT/SIGTERM/SIGHUP handler; game was not launched")?;
    Ok(flag)
}

fn terminate_wine_prefix(runtime: &ResolvedRuntime, pfx_dir: &Path, child: &mut OwnedChild) {
    // wineserver is scoped by WINEPREFIX. A shared Wine path or executable name is not
    // evidence of ownership: other games can be using that same runtime.
    let wineserver_bin = &runtime.server;
    if let Err(error) = run_wineserver_helper(
        wineserver_bin,
        pfx_dir,
        "-k",
        Duration::from_millis(1500),
        Some(runtime),
    ) {
        log_runner(&format!("Wineserver shutdown failed: {error:#}"));
    }
    if let Err(error) = child.terminate(TERMINATION_GRACE) {
        log_runner(&format!("Launcher group shutdown failed: {error:#}"));
    }
    wait_wineserver(runtime, pfx_dir, 1000);
}

fn stop_game(
    session: &RunSession,
    runtime: &ResolvedRuntime,
    prefix: &Path,
    child: &mut OwnedChild,
    cleanup: &mut PrefixCleanup,
) {
    session.clock.begin_cleanup();
    terminate_wine_prefix(runtime, prefix, child);
    cleanup.armed = false;
}

/// Declared after the prefix lease, so failed preparation stops this prefix's
/// server before ownership is released. Normal game shutdown already does this.
struct PrefixCleanup {
    runtime: ResolvedRuntime,
    prefix: PathBuf,
    clock: session::Clock,
    armed: bool,
}
impl Drop for PrefixCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.clock.begin_cleanup();
        if let Err(error) = run_wineserver_helper(
            &self.runtime.server,
            &self.prefix,
            "-k",
            Duration::from_secs(2),
            Some(&self.runtime),
        ) {
            log_runner(&format!("Prefix preparation cleanup failed: {error:#}"));
        }
        wait_wineserver(&self.runtime, &self.prefix, 1000);
    }
}

pub fn is_wine_game_process_line(
    line: &str,
    my_pid: u32,
    runner_dir_str: &str,
    target_name: &str,
) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut parts = trimmed.split_whitespace();
    let pid_str = parts.next().unwrap_or("");
    if let Ok(pid) = pid_str.parse::<u32>()
        && pid == my_pid
    {
        return false;
    }

    let args_part = trimmed[pid_str.len()..].trim().to_lowercase();

    // Ignore nucleon-runner and nucleon CLI
    if args_part.contains("nucleon-runner") || args_part.contains("/nucleon ") {
        return false;
    }

    // Exclude Wine infrastructure, services, and crash report utilities
    if args_part.contains("wineserver")
        || args_part.contains("winedevice.exe")
        || args_part.contains("services.exe")
        || args_part.contains("plugplay.exe")
        || args_part.contains("svchost.exe")
        || args_part.contains("rpcss.exe")
        || args_part.contains("explorer.exe")
        || args_part.contains("conhost.exe")
        || args_part.contains("steam.exe")
        || args_part.contains("wineboot.exe")
        || args_part.contains("rundll32.exe")
        || args_part.contains("winemenubuilder.exe")
        || args_part.contains("mscorsvw.exe")
        || args_part.contains("winecfg.exe")
        || args_part.contains("regsvr32.exe")
        || args_part.contains("reg.exe")
        || args_part.contains("crashsender")
        || args_part.contains("crashpad_handler")
        || args_part.contains("unitycrashhandler")
    {
        return false;
    }

    let matches_runner = args_part.contains(runner_dir_str)
        || args_part.contains("wine64-preloader")
        || args_part.contains("wine-preloader")
        || args_part.contains("wine64")
        || args_part.contains("/wine");

    let matches_target = !target_name.is_empty() && args_part.contains(target_name);

    if !target_name.is_empty() {
        matches_target || (matches_runner && args_part.contains(".exe"))
    } else {
        matches_runner && args_part.contains(".exe")
    }
}

pub fn wine_process_pids(prefix: &Path) -> Vec<i32> {
    let prefix_env = std::ffi::OsString::from(format!("WINEPREFIX={}", prefix.display()));
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::everything(),
    );
    sys.processes()
        .iter()
        .filter(|(_, process)| process.environ().contains(&prefix_env))
        .map(|(pid, _)| pid.as_u32() as i32)
        .collect()
}

pub fn is_wine_game_process_running(runner_dir: &Path, target_exe: &Path, prefix: &Path) -> bool {
    is_wine_process_running(runner_dir, target_exe, prefix, false)
}

fn is_wine_process_running(
    runner_dir: &Path,
    target_exe: &Path,
    prefix: &Path,
    standalone: bool,
) -> bool {
    let prefix_env = format!("WINEPREFIX={}", prefix.display());
    let runner = runner_dir.to_string_lossy().to_lowercase();
    let target = target_exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::everything(),
    );
    sys.processes().iter().any(|(pid, process)| {
        if !process
            .environ()
            .iter()
            .any(|value| value == &std::ffi::OsString::from(&prefix_env))
        {
            return false;
        }
        let command = process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let line = format!(
            "{} {} {}",
            pid.as_u32(),
            process.name().to_string_lossy(),
            command
        );
        if standalone {
            standalone::is_application_process_line(&line, std::process::id(), &runner, &target)
        } else {
            is_wine_game_process_line(&line, std::process::id(), &runner, &target)
        }
    })
}

struct RunOutcome {
    code: i32,
    status: &'static str,
}

pub fn run_with_args(args: &[String]) -> Result<()> {
    if args
        .get(1)
        .is_some_and(|verb| verb == "check-app-compatibility")
    {
        return Ok(());
    }
    let mut session = RunSession::create(args)?;
    log_runner(&format!("Launch logs: {}", session.directory.display()));
    let result = session
        .configure()
        .and_then(|()| run_with_args_inner(args, &mut session));
    finish_run(&mut session, result)
}

fn finish_run(session: &mut RunSession, result: Result<RunOutcome>) -> Result<()> {
    match result {
        Ok(outcome) => {
            session.finish(outcome.status, Some(outcome.code), None)?;
            log_runner(&format!(
                "Launch finished: {} (exit {})",
                outcome.status, outcome.code
            ));
            if outcome.code != 0 {
                std::process::exit(outcome.code);
            }
            Ok(())
        }
        Err(error) => {
            log_runner(&format!("Launch failed: {error:#}"));
            if let Err(report_error) = session.finish("failed", None, Some(format!("{error:#}"))) {
                eprintln!("Cannot finalize launch receipt: {report_error:#}");
            }
            Err(error)
        }
    }
}

fn run_with_args_inner(args: &[String], session: &mut RunSession) -> Result<RunOutcome> {
    if args.len() < 2 {
        bail!("Usage: nucleon-runner <verb> [args...]");
    }
    let verb = &args[1];
    for arg in args {
        if arg.contains("iscriptevaluator.exe") {
            log_runner("Intercepted iscriptevaluator.exe, exiting cleanly");
            return Ok(RunOutcome {
                code: 0,
                status: "skipped",
            });
        }
    }

    // Parse Steam compatibility environment
    let compat_appid = session.appid.clone();
    let compat_data_path = env::var_os("STEAM_COMPAT_DATA_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::support_dir().join("default_prefix"));
    // WINEPREFIX is deliberately not exported into this process' environment: every Wine and
    // wineserver invocation receives it explicitly in its own environment map, which keeps the
    // value from leaking to unrelated children (and mutating env while other threads read it).

    // Determine target executable and arguments
    // Steam invokes: nucleon-runner run <exe_path> [game_args...]
    let game_args: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        vec![]
    };

    if game_args.is_empty() {
        log_runner("No game executable specified to run, exiting.");
        return Ok(RunOutcome {
            code: 0,
            status: "skipped",
        });
    }

    let target_exe = PathBuf::from(&game_args[0]);

    // The app id reaches the game only through its environment map (SteamAppId/SteamGameId below).
    // Writing `steam_appid.txt` next to the executable must never be done: it switches Steamworks
    // into standalone mode, which Denuvo-protected titles reject by raising a modal
    // "Application load error 3:0000065432" dialog during startup. The game then blocks forever in
    // that dialog with no input pump, presenting as an engine-independent hang before Direct3D is
    // ever touched. It also silently mutates a Steam-managed game directory.

    let explicit_profile = config::environment("NUCLEON_GAME_PROFILE")?.map(PathBuf::from);
    let override_path = compat_appid
        .as_ref()
        .map(|id| paths::support_dir().join(format!("launch_override_{id}.json")));
    let override_source = override_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "launch override".into());
    let launch_override = if explicit_profile.is_some() {
        None
    } else {
        override_path
            .as_deref()
            .map(load_launch_settings)
            .transpose()?
            .flatten()
    };
    let cli_request = launch_override
        .as_ref()
        .is_some_and(|p| p.wine.is_some() || p.emulator.is_some());
    let environment_wine = if cli_request {
        None
    } else {
        ["NUCLEON_WINE", "NUCLEON_WINE_PATH", "WINE_PATH"]
            .iter()
            .map(|key| config::environment(key).map(|value| value.map(|v| ((*key).to_string(), v))))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .next()
    };
    let emulator_env = if cli_request {
        None
    } else {
        config::environment("NUCLEON_EMULATOR")?
    };
    let emulator = launch_override
        .as_ref()
        .and_then(|p| p.emulator)
        .or(emulator_env
            .as_deref()
            .map(str::parse)
            .transpose()
            .context("Invalid NUCLEON_EMULATOR")?);
    let wine_request = launch_override
        .as_ref()
        .and_then(|p| p.wine.as_deref())
        .or(environment_wine.as_ref().map(|(_, value)| value.as_str()));
    let request_source = environment_wine
        .as_ref()
        .map(|(name, _)| name.as_str())
        .unwrap_or(&override_source);
    let explicit_runtime = nucleon_core::runtime_selection::resolve(wine_request, emulator)
        .with_context(|| format!("Invalid runtime request from {request_source}"))?;
    let explicit_runtime_request = explicit_runtime.is_some();
    // An explicit launch request supersedes tool defaults, including a stale FEX pin.
    let fex_selection = if explicit_runtime_request {
        None
    } else {
        config::environment(nucleon_core::steam::fex::SELECTION_ENV)?
            .map(|path| nucleon_core::steam::fex::load_selection(Path::new(&path)))
            .transpose()?
    };
    let mut selected_runtime = explicit_runtime.or(fex_selection);
    let mut is_fex = selected_runtime
        .as_ref()
        .is_some_and(|r| r.mode == nucleon_core::runtime::ExecutionMode::Arm64Fex);
    let loaded_native_profile = is_fex;
    let mut profile_path = explicit_profile.clone().or_else(|| {
        compat_appid
            .as_ref()
            .map(|id| paths::game_profiles_dir(is_fex).join(format!("{id}.json")))
    });
    let mut profile_source = profile_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "game profile".into());
    let loaded_profile = profile_path
        .as_deref()
        .map(load_launch_settings)
        .transpose()?
        .flatten();
    if explicit_profile.is_some() && loaded_profile.is_none() {
        bail!("Explicit game profile was not found: {profile_source}");
    }
    let mut game_profile = loaded_profile.unwrap_or_default();
    let wine_override_root = if let Some(runtime) = &selected_runtime {
        if !explicit_runtime_request && let Some(query) = &game_profile.wine {
            let actual = nucleon_core::runtime_selection::named(query)?;
            if actual.root != runtime.root {
                bail!(
                    "ARM64/FEX Steam tool pins {}. Remove the conflicting wine field from {} or register the intended native runtime",
                    runtime.root.display(),
                    profile_source
                );
            }
        }
        Some(runtime.root.clone())
    } else {
        let root = if game_profile.emulator.is_some() {
            nucleon_core::runtime_selection::resolve(
                game_profile.wine.as_deref(),
                game_profile.emulator,
            )?
            .map(|r| r.root)
        } else {
            resolve_wine_override(&game_profile, &profile_source)?
        };
        if let Some(root) = &root {
            let runtime = ResolvedRuntime::inspect(root)?;
            is_fex = runtime.mode == nucleon_core::runtime::ExecutionMode::Arm64Fex;
            selected_runtime = Some(runtime);
        }
        root
    };
    // A legacy ordinary profile may select a native runtime. Its graphics settings
    // come from the native namespace, exactly as with an explicit CLI selection.
    if is_fex && !loaded_native_profile && explicit_profile.is_none() {
        profile_path = compat_appid
            .as_ref()
            .map(|id| paths::game_profiles_dir(true).join(format!("{id}.json")));
        profile_source = profile_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "game profile".into());
        game_profile = profile_path
            .as_deref()
            .map(load_launch_settings)
            .transpose()?
            .flatten()
            .unwrap_or_default();
    }
    if is_fex {
        game_profile = game_profile.with_defaults(LaunchOverride::fex_default(
            compat_appid.as_deref().unwrap_or(""),
        ));
    }
    let engine_env = if launch_override
        .as_ref()
        .and_then(|o| o.engine.as_ref())
        .is_none()
    {
        config::environment("NUCLEON_ENGINE")?
    } else {
        None
    };
    let requested_engine = requested_engine(
        launch_override.as_ref(),
        &override_source,
        engine_env.as_deref(),
        &game_profile,
        &profile_source,
    )?;
    let configured_backend = if matches!(
        requested_engine,
        None | Some(nucleon_core::detector::TargetEngine::Auto)
    ) {
        if (is_fex || explicit_runtime_request)
            && env::var_os("NUCLEON_BACKEND").is_none()
            && env::var_os("NUCLEON_GRAPHICS_BACKEND").is_none()
        {
            nucleon_core::backend::GraphicsBackend::Auto
        } else {
            nucleon_core::backend::resolve_active_backend_with_origin()?.0
        }
    } else {
        nucleon_core::backend::GraphicsBackend::Auto
    };
    validate_graphics_paths(
        &game_profile,
        &profile_source,
        selected_runtime.as_ref(),
        requested_engine,
    )?;
    let enable_hud = match launch_override
        .as_ref()
        .and_then(|o| o.hud)
        .or(game_profile.hud)
    {
        Some(value) => value,
        None => environment_boolean("MTL_HUD_ENABLED")?.unwrap_or(false),
    };
    let enable_overlay = if game_profile.overlay == Some(false) {
        false
    } else {
        environment_boolean("NUCLEON_OVERLAY")?
            .or(game_profile.overlay)
            .unwrap_or(false)
    };
    let user_shim_override = match launch_override.as_ref().and_then(|o| o.use_steam_shim) {
        Some(value) => Some(value),
        None => environment_boolean("NUCLEON_STEAM_SHIM")?.or(game_profile.use_steam_shim),
    };

    let mut resolved_graphics = None;
    let mut resolved_runtime = selected_runtime.clone();
    let detection = nucleon_core::detector::detect_target_engine(&target_exe);
    let mut effective_profile = game_profile.clone();
    effective_profile.upscaler = requested_upscaler(
        launch_override.as_ref(),
        if launch_override
            .as_ref()
            .is_some_and(|p| p.upscaler.is_some())
        {
            None
        } else {
            config::environment("NUCLEON_UPSCALER")?
        }
        .as_deref(),
        &game_profile,
    )?;
    effective_profile.graphics_api = launch_override
        .as_ref()
        .and_then(|p| p.graphics_api)
        .or(game_profile.graphics_api);
    effective_profile.steam_protocol = launch_override
        .as_ref()
        .and_then(|p| p.steam_protocol)
        .or(game_profile.steam_protocol);
    let graphics_api = effective_profile.graphics_api(&target_exe, &detection.api)?;
    let (engine, api_desc) = match requested_engine {
        Some(nucleon_core::detector::TargetEngine::Auto) | None => {
            let custom_wine = explicit_runtime_request
                || is_fex
                || game_profile.wine.is_some()
                || ["NUCLEON_WINE", "NUCLEON_WINE_PATH", "WINE_PATH"]
                    .iter()
                    .any(|key| env::var_os(key).is_some());
            let wine_engine = select_automatic_engine(
                &graphics_api,
                {
                    let root = wine_override_root
                        .clone()
                        .or_else(|| nucleon_core::wine::get_active_wine_runtime().map(|r| r.root))
                        .or_else(runner::find_gptk_runner);
                    let icd = if env::var_os("VK_DRIVER_FILES").is_none()
                        && env::var_os("VK_ICD_FILENAMES").is_none()
                    {
                        game_profile.vulkan_icd.as_deref()
                    } else {
                        None
                    };
                    let dxvk = if env::var_os("DXVK_PATH").is_none() {
                        game_profile.dxvk_path.as_deref()
                    } else {
                        None
                    };
                    root.map(|root| -> Result<_> {
                        let runtime = match &selected_runtime {
                            Some(pinned) => pinned.clone(),
                            None => ResolvedRuntime::inspect(&root)?,
                        };
                        let components = GraphicsComponents::discover(&runtime, icd, dxvk)?;
                        let available = components.available(
                            &runtime,
                            &graphics_api,
                            nucleon_core::detector::is_pe_file_64_bit(
                                detection.binary.as_deref().unwrap_or(&target_exe),
                            ),
                        );
                        resolved_graphics = Some(components);
                        resolved_runtime = Some(runtime);
                        Ok(available)
                    })
                    .transpose()?
                    .unwrap_or_default()
                },
                configured_backend,
                custom_wine,
            )?;
            trace_runner(&format!(
                "Automatic Runner: routing graphics API {:?} (detected {:?}, found: {:?}, evidence: {}) -> selecting {:?}",
                graphics_api,
                detection.api,
                detection.detected_dll,
                detection.evidence.description(),
                wine_engine
            ));
            (
                wine_engine,
                format!(
                    "Automatic Detection: {:?} (DLL: {:?}) -> {}",
                    detection.api,
                    detection.detected_dll,
                    wine_engine.display_name()
                ),
            )
        }
        Some(eng) => {
            trace_runner(&format!("Engine manually overridden -> {:?}", eng));
            (eng, format!("Manual Override ({:?})", eng))
        }
    };

    if explicit_runtime_request && engine == nucleon_core::detector::TargetEngine::Gptk {
        bail!(
            "An explicit Wine selection cannot use the separate GPTK runner; choose auto, dxmt, dxvk, vkd3d, kosmickrisp or wine"
        );
    }
    if is_fex && engine == nucleon_core::detector::TargetEngine::Gptk {
        bail!(
            "ARM64/FEX cannot use the Intel GPTK engine. Choose auto, dxmt, vkd3d, dxvk or wine in its profile"
        );
    }
    log_runner(&format!(
        "Engine Router: target '{}' [{}] -> engine {:?}",
        target_exe.display(),
        api_desc,
        engine
    ));

    // Per-game Wine is already resolved; use it for execution as well as preflight.
    let (runner_dir, active_engine) = if engine != nucleon_core::detector::TargetEngine::Gptk
        && let Some(root) = wine_override_root
    {
        (root, engine)
    } else {
        runner::resolve_runner_for_engine(engine).with_context(|| {
            format!(
                "Cannot prepare {}. Check 'nucleon wine status' and 'nucleon backends list'",
                engine.display_name()
            )
        })?
    };

    let mut runtime = match resolved_runtime {
        Some(runtime) if runtime.root == runner_dir.canonicalize()? => runtime,
        _ => ResolvedRuntime::inspect(&runner_dir)?,
    };
    nucleon_core::runtime_selection::check_host(&runtime)?;
    effective_profile.apply_protocol(&mut runtime)?;
    let pfx_dir = paths::game_prefix(&compat_data_path, &runtime);
    let graphics = match resolved_graphics {
        Some(components) => components,
        None => GraphicsComponents::discover(
            &runtime,
            game_profile.vulkan_icd.as_deref(),
            game_profile.dxvk_path.as_deref(),
        )?,
    };
    let plan = LaunchPlan::new(
        runtime,
        pfx_dir.clone(),
        active_engine,
        graphics,
        nucleon_core::detector::is_pe_file_64_bit(&target_exe),
        user_shim_override,
        &graphics_api,
    )?;
    execute_launch(
        PreparedLaunch {
            plan,
            data_path: compat_data_path,
            appid: compat_appid,
            arguments: game_args,
            cwd: None,
            hud: enable_hud,
            overlay: enable_overlay,
            wait: verb == "waitforexitandrun",
            consume_override: override_path.filter(|_| launch_override.is_some()),
            upscaler: effective_profile.upscaler,
            environment: Default::default(),
        }
        .resolve()?,
        session,
    )
}

struct PreparedLaunch {
    environment: std::collections::BTreeMap<String, String>,
    upscaler: Option<nucleon_core::upscaler::Settings>,
    plan: LaunchPlan,
    data_path: PathBuf,
    appid: Option<String>,
    arguments: Vec<String>,
    cwd: Option<PathBuf>,
    hud: bool,
    overlay: bool,
    wait: bool,
    consume_override: Option<PathBuf>,
}

/// Side-effect-free decisions shared by preview and execution. Installation state is
/// checked again under its lock when execution begins.
struct ResolvedLaunch {
    prepared: PreparedLaunch,
    scaler_root: PathBuf,
    scaler_choice: nucleon_core::upscaler::Settings,
    resolved_scaler: nucleon_core::upscaler::Resolved,
}

impl PreparedLaunch {
    fn resolve(self) -> Result<ResolvedLaunch> {
        let scaler_root = nucleon_core::upscaler::root();
        let scaler_choice = self.upscaler.clone().unwrap_or_else(|| {
            nucleon_core::upscaler::Settings::selected(nucleon_core::upscaler::Backend::Game)
        });
        let resolved_scaler = nucleon_core::upscaler::resolve_for_launch(
            &scaler_root,
            &self.plan,
            Path::new(&self.arguments[0]),
            &scaler_choice,
        )?;
        if self.upscaler.is_some() || scaler_choice.backend != nucleon_core::upscaler::Backend::Game
        {
            nucleon_core::upscaler::preview(&scaler_root, &resolved_scaler)?;
        }
        nucleon_core::upscaler::validate_raw_overrides(
            config::environment("NUCLEON_WINEDLLOVERRIDES")?
                .as_deref()
                .unwrap_or(""),
            scaler_choice.backend,
        )?;
        Ok(ResolvedLaunch {
            prepared: self,
            scaler_root,
            scaler_choice,
            resolved_scaler,
        })
    }
}

fn execute_launch(resolved: ResolvedLaunch, session: &mut RunSession) -> Result<RunOutcome> {
    let ResolvedLaunch {
        prepared,
        scaler_root,
        scaler_choice,
        resolved_scaler,
    } = resolved;
    let PreparedLaunch {
        upscaler,
        mut plan,
        data_path: compat_data_path,
        appid: compat_appid,
        arguments: game_args,
        cwd,
        hud: enable_hud,
        overlay: enable_overlay,
        wait,
        consume_override,
        environment,
    } = prepared;
    let target_exe = PathBuf::from(&game_args[0]);
    if upscaler.is_some() {
        session.upscaler_requested(&scaler_choice)?;
    }
    let scaler_installation = if scaler_choice.backend != nucleon_core::upscaler::Backend::Game {
        Some(nucleon_core::upscaler::Installation::open(
            &scaler_root,
            &resolved_scaler,
        )?)
    } else {
        nucleon_core::upscaler::Installation::existing(&scaler_root, &target_exe)?
    };
    if let Some(installation) = &scaler_installation {
        installation.assert_idle(Some(&plan.prefix))?;
    }
    let runner_dir = plan.runtime.root.clone();
    let pfx_dir = plan.prefix.clone();
    let active_engine = plan.engine;
    plan.log_directory = Some(session.directory.clone());
    plan.diagnostics = session.debug;
    session.resolved(&plan, None, None)?;
    let run_seconds = config::environment("NUCLEON_RUN_SECONDS")?
        .map(|value| {
            value
                .parse::<std::num::NonZeroU64>()
                .map(|n| Duration::from_secs(n.get()))
                .with_context(|| "NUCLEON_RUN_SECONDS must be a positive number of seconds")
        })
        .transpose()?;
    let wine_bin = plan.runtime.loader.clone();
    trace_runner(&format!(
        "Resolved runtime mode: {:?}; native libraries: {}",
        plan.runtime.mode,
        plan.runtime.unix_dir.display()
    ));

    if let Some(path) = consume_override {
        fs::remove_file(&path)
            .with_context(|| format!("Cannot consume launch override {}", path.display()))?;
    }

    let term_flag = termination_flag()?;
    let _runtime_lease = nucleon_core::managed_runtime::lease(&plan.runtime.root)?;
    let _prefix_session = if !plan.steam_integration
        && plan.prefix != paths::game_prefix(&compat_data_path, &plan.runtime)
    {
        PrefixSession::acquire_existing(&pfx_dir, &plan.runtime, || {
            term_flag.load(Ordering::SeqCst)
        })?
    } else {
        PrefixSession::acquire(
            &compat_data_path,
            &plan.runtime,
            if wait {
                Duration::from_secs(5)
            } else {
                Duration::ZERO
            },
            || term_flag.load(Ordering::SeqCst),
        )?
    };
    let mut prefix_cleanup = PrefixCleanup {
        runtime: plan.runtime.clone(),
        prefix: pfx_dir.clone(),
        clock: session.clock.clone(),
        armed: true,
    };
    log_runner(&format!(
        "Prefix: {} (family {}, ABI {})",
        pfx_dir.display(),
        plan.runtime.prefix.family,
        plan.runtime.prefix.abi
    ));
    // Wait for previous instance if requested
    if wait {
        wait_wineserver(&plan.runtime, &pfx_dir, 5000);
    }

    let mut observed_binding = None;
    let mut exec_env = plan.execution_env(enable_hud, enable_overlay);
    plan.configure_launch_environment(&mut exec_env)?;
    if let Some(installation) = &scaler_installation {
        let binding =
            nucleon_core::upscaler::prepare(&scaler_root, installation, &resolved_scaler)?;
        nucleon_core::upscaler::apply_environment(&binding, &mut exec_env)?;
        session.upscaler(&binding)?;
        nucleon_core::upscaler::begin_observation(&binding)?;
        observed_binding = Some(binding);
    }
    plan.prepare_prefix().with_context(|| {
        format!(
            "Cannot initialize prefix {} using Wine {}",
            pfx_dir.display(),
            runner_dir.display()
        )
    })?;
    let staged = plan.stage_graphics()?;
    log_runner(&format!(
        "Resolved graphics {:?} active ({staged} DLL operations)",
        active_engine
    ));
    let caches = nucleon_core::shader_cache::ShaderCaches::prepare(
        &plan,
        &compat_data_path,
        compat_appid.as_deref(),
        &target_exe,
        &exec_env,
    )?;
    if let Some(caches) = &caches {
        caches.apply(&mut exec_env);
    }
    session.resolved(
        &plan,
        Some(staged),
        caches.as_ref().map(serde_json::to_value).transpose()?,
    )?;
    if let Some(directory) = target_exe.parent() {
        let helpers = nucleon_core::detector::bundled_directx_helpers(
            directory,
            nucleon_core::detector::is_pe_file_64_bit(&target_exe),
        );
        if !helpers.is_empty() {
            let overrides = exec_env.entry("WINEDLLOVERRIDES".into()).or_default();
            if !overrides.is_empty() {
                overrides.push(';');
            }
            overrides.push_str(&format!("{}=n,b", helpers.join(",")));
            log_runner(&format!(
                "Using bundled native DirectX helpers: {}",
                helpers.join(", ")
            ));
        }
    }
    if let Some(ref id) = compat_appid {
        exec_env.insert("SteamAppId".to_string(), id.clone());
        exec_env.insert("SteamGameId".to_string(), id.clone());
    }

    if term_flag.load(Ordering::SeqCst) {
        return Ok(RunOutcome {
            code: 0,
            status: "cancelled",
        });
    }

    // Working directory is the parent of the exe if available
    let game_cwd = cwd.or_else(|| {
        target_exe
            .parent()
            .filter(|p| p.is_dir())
            .map(Path::to_path_buf)
    });

    let is_64 = nucleon_core::detector::is_pe_file_64_bit(&target_exe);

    let steamworks_title = nucleon_core::detector::file_uses_steamworks(&target_exe);
    let wine_args = plan.wine_args(&game_args)?;
    trace_runner(&format!(
        "Steam client handshake: target {} Steamworks; 64-bit {}; wrapper {}",
        if steamworks_title {
            "links"
        } else {
            "does not link"
        },
        if is_64 { "yes" } else { "no" },
        if wine_args.first().is_some_and(|a| a.contains("steam.exe")) {
            "enabled"
        } else {
            "disabled"
        }
    ));

    trace_runner(&format!("Wine binary: {}", wine_bin.display()));
    trace_runner(&format!("Game CWD: {:?}", game_cwd));
    trace_runner(&format!(
        "Target architecture: {}",
        if is_64 { "64-bit" } else { "32-bit" }
    ));
    trace_runner(&format!(
        "Launching game with Wine: {:?}",
        nucleon_core::launch_profile::redact_arguments(&wine_args)
    ));

    for key in [
        "STEAM_COMPAT_APP_ID",
        "STEAM_COMPAT_DATA_PATH",
        "STEAM_COMPAT_INSTALL_PATH",
        "STEAM_COMPAT_CLIENT_INSTALL_PATH",
        "SteamAppId",
        "SteamGameId",
        "NUCLEON_ENGINE",
        "NUCLEON_WINE",
        "NUCLEON_WINE_PATH",
        "NUCLEON_STEAM_SHIM",
        "WINEARCH",
        "WINEPREFIX",
        "WINEDLLPATH",
        "WINEDLLOVERRIDES",
        "DYLD_FALLBACK_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
    ] {
        if let Ok(value) = env::var(key) {
            trace_runner(&format!("INHERITED_ENV: {key}={value}"));
        }
    }

    let mut cmd = plan.runtime.command(&wine_bin);
    cmd.envs(&environment);
    nucleon_core::launch::clean_command(&mut cmd);
    if scaler_installation.is_some() || upscaler.is_some() {
        let keys = env::vars_os()
            .map(|(key, _)| key)
            .chain(environment.keys().map(std::ffi::OsString::from))
            .collect::<Vec<_>>();
        for key in keys {
            let key_text = key.to_string_lossy();
            if key_text.starts_with("FSR4_")
                || key_text == "D3DMETAL_FRAMEWORK_PATH"
                || key_text == "D3DM_ENABLE_METALFX"
            {
                cmd.env_remove(key);
            }
        }
    }
    if !plan.steam_integration {
        nucleon_core::launch::clean_standalone_command(&mut cmd);
    }
    cmd.env_remove("NUCLEON_LAUNCH_REQUEST");
    cmd.args(&wine_args);
    // Clear parent injection; exec_env supplies only the native renderer on opt-in.
    cmd.env_remove("DYLD_INSERT_LIBRARIES")
        .env_remove("STEAM_DYLD_INSERT_LIBRARIES");

    for (k, v) in &exec_env {
        trace_runner(&format!("ENV: {}={}", k, v));
        cmd.env(k, v);
    }

    if let Some(ref cwd) = game_cwd {
        cmd.current_dir(cwd);
    }

    let file = fs::File::create(&session.wine_log)
        .with_context(|| format!("Cannot write Wine log {}", session.wine_log.display()))?;
    cmd.stdout(std::process::Stdio::from(file.try_clone()?));
    cmd.stderr(std::process::Stdio::from(file));

    let mut child = OwnedChild::spawn(&mut cmd).with_context(|| format!("Cannot launch Wine {}. Check that this binary is executable and matches your Mac's architecture", wine_bin.display()))?;

    session.started()?;
    let started = std::time::Instant::now();

    // Supervision watchdog
    let mut primary_exited = false;
    let mut exit_code = 0;
    let mut outcome = "exited";

    loop {
        if let Some(binding) = &observed_binding {
            session.upscaler_observation(nucleon_core::upscaler::observation(binding)?)?;
        }
        if run_seconds.is_some_and(|limit| started.elapsed() >= limit) {
            log_runner("Bounded run finished; stopping owned Wine prefix");
            stop_game(
                session,
                &plan.runtime,
                &pfx_dir,
                &mut child,
                &mut prefix_cleanup,
            );
            exit_code = 0;
            outcome = "bounded";
            break;
        }
        if term_flag.load(Ordering::SeqCst) {
            log_runner("Termination signal received, stopping Wine processes cleanly");
            stop_game(
                session,
                &plan.runtime,
                &pfx_dir,
                &mut child,
                &mut prefix_cleanup,
            );
            exit_code = 0;
            outcome = "cancelled";
            break;
        }

        if !primary_exited {
            match child.try_wait() {
                Ok(Some(status)) => {
                    primary_exited = true;
                    exit_code = status.code().unwrap_or(1);
                    session.primary_exited(status)?;
                    log_runner(&format!("Wine primary process exited with: {:?}", status));

                    // When Wine acts as a launcher that spawns/forks wine-preloader and exits,
                    // poll for up to 5 seconds to detect the background Wine game process before deciding to exit.
                    let mut found_game = false;
                    for _ in 0..25 {
                        if is_wine_process_running(
                            &runner_dir,
                            &target_exe,
                            &pfx_dir,
                            !plan.steam_integration,
                        ) {
                            found_game = true;
                            break;
                        }
                        thread::sleep(Duration::from_millis(200));
                    }

                    if found_game {
                        log_runner("Detected active Wine game process; continuing supervision");
                        if plan.steam_integration {
                            exit_code = 0;
                        }
                    } else {
                        log_runner(
                            "No Wine game processes detected after primary launcher exit, shutting down",
                        );
                        stop_game(
                            session,
                            &plan.runtime,
                            &pfx_dir,
                            &mut child,
                            &mut prefix_cleanup,
                        );
                        break;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    log_runner(&format!("Error waiting on Wine process: {}", e));
                    exit_code = 1;
                    outcome = "supervision-failed";
                    stop_game(
                        session,
                        &plan.runtime,
                        &pfx_dir,
                        &mut child,
                        &mut prefix_cleanup,
                    );
                    break;
                }
            }
        } else {
            // Check if any background Wine game processes are still running.
            // Require two consecutive negative checks (500ms apart) to avoid false positives during process state changes.
            if !is_wine_process_running(&runner_dir, &target_exe, &pfx_dir, !plan.steam_integration)
            {
                thread::sleep(Duration::from_millis(500));
                if !is_wine_process_running(
                    &runner_dir,
                    &target_exe,
                    &pfx_dir,
                    !plan.steam_integration,
                ) {
                    log_runner("All Wine game processes have terminated, exiting cleanly");
                    stop_game(
                        session,
                        &plan.runtime,
                        &pfx_dir,
                        &mut child,
                        &mut prefix_cleanup,
                    );
                    break;
                }
            }
        }

        thread::sleep(Duration::from_millis(500));
    }

    Ok(RunOutcome {
        code: exit_code,
        status: outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sysinfo::{Pid, Signal};

    #[test]
    fn upscaler_precedence_preserves_explicit_game_and_reports_invalid_values() {
        use nucleon_core::upscaler::{Backend, Settings};
        let profile = LaunchOverride {
            upscaler: Some(Settings::selected(Backend::Fsr2)),
            ..Default::default()
        };
        let temporary = LaunchOverride {
            upscaler: Some(Settings::selected(Backend::Game)),
            ..Default::default()
        };
        assert_eq!(
            requested_upscaler(Some(&temporary), Some("bad"), &profile)
                .unwrap()
                .unwrap()
                .backend,
            Backend::Game
        );
        assert_eq!(
            requested_upscaler(None, Some("fsr3.1"), &profile)
                .unwrap()
                .unwrap()
                .backend,
            Backend::Fsr31
        );
        assert!(requested_upscaler(None, Some("bad"), &profile).is_err());
        assert_eq!(
            requested_upscaler(None, None, &profile)
                .unwrap()
                .unwrap()
                .backend,
            Backend::Fsr2
        );
    }

    #[test]
    fn helper_timeout_kills_and_reaps_the_child() {
        let mut child = OwnedChild::spawn(Command::new("sleep").arg("30")).unwrap();
        let pid = Pid::from_u32(child.id());
        assert!(
            wait_child_with_timeout(&mut child, Duration::from_millis(10))
                .unwrap()
                .is_none()
        );
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        assert!(
            system.process(pid).is_none(),
            "Timed-out helper must not remain as a zombie"
        );
    }

    #[cfg(unix)]
    #[test]
    fn helper_wait_preserves_exit_status_and_prefix_scope() {
        use std::os::unix::fs::PermissionsExt;
        let mut child = OwnedChild::spawn(Command::new("sh").args(["-c", "exit 7"])).unwrap();
        assert_eq!(
            wait_child_with_timeout(&mut child, Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .code(),
            Some(7)
        );
        let dir = tempfile::tempdir().unwrap();
        let server = dir.path().join("wineserver");
        let prefix = dir.path().join("prefix with spaces");
        fs::create_dir(&prefix).unwrap();
        fs::write(
            &server,
            "#!/bin/sh\nprintf '%s\\n' \"$WINEPREFIX\" \"$1\" > \"$WINEPREFIX/helper-args\"\n",
        )
        .unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
        for argument in ["-k", "-w"] {
            run_wineserver_helper(&server, &prefix, argument, Duration::from_secs(2), None)
                .unwrap();
            assert_eq!(
                fs::read_to_string(prefix.join("helper-args")).unwrap(),
                format!("{}\n{argument}\n", prefix.display())
            );
        }
    }

    // Each worker runs in its own test process, keeping ctrlc's global handler out of other tests.
    #[test]
    fn signal_handler_worker() {
        let Ok(ready) = env::var("NUCLEON_TEST_SIGNAL_READY") else {
            return;
        };
        let flag = termination_flag().unwrap();
        if env::var_os("NUCLEON_TEST_DUPLICATE_HANDLER").is_some() {
            let error = termination_flag().unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Cannot install SIGINT/SIGTERM/SIGHUP handler")
            );
            return;
        }
        fs::write(ready, b"ready").unwrap();
        let start = std::time::Instant::now();
        while !flag.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            flag.load(Ordering::SeqCst),
            "Termination signal was not handled"
        );
    }

    #[cfg(unix)]
    #[test]
    fn termination_signals_reach_the_handler_and_registration_failures_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        for signal in [Signal::Interrupt, Signal::Term, Signal::Hangup] {
            let _ = fs::remove_file(&ready);
            let mut command = Command::new(env::current_exe().unwrap());
            command
                .args(["--exact", "tests::signal_handler_worker", "--nocapture"])
                .env("NUCLEON_TEST_SIGNAL_READY", &ready)
                .env_remove("NUCLEON_TEST_DUPLICATE_HANDLER");
            let mut child = OwnedChild::spawn(&mut command).unwrap();
            let start = std::time::Instant::now();
            while !ready.is_file() && start.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(10));
            }
            if !ready.is_file() {
                let _ = child.terminate(Duration::ZERO);
                panic!("Signal worker did not become ready");
            }
            let pid = Pid::from_u32(child.id());
            let mut system = System::new();
            system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            let sent = system
                .process(pid)
                .and_then(|process| process.kill_with(signal));
            let status = wait_child_with_timeout(&mut child, Duration::from_secs(6)).unwrap();
            assert_eq!(sent, Some(true));
            assert!(
                status.is_some_and(|status| status.success()),
                "Signal worker failed for {signal:?}"
            );
        }
        let mut command = Command::new(env::current_exe().unwrap());
        command
            .args(["--exact", "tests::signal_handler_worker", "--nocapture"])
            .env("NUCLEON_TEST_SIGNAL_READY", &ready)
            .env("NUCLEON_TEST_DUPLICATE_HANDLER", "1");
        let mut child = OwnedChild::spawn(&mut command).unwrap();
        assert!(
            wait_child_with_timeout(&mut child, Duration::from_secs(6))
                .unwrap()
                .is_some_and(|status| status.success())
        );
    }

    #[test]
    fn engine_precedence_preserves_explicit_auto_and_rejects_invalid_winners() {
        use nucleon_core::detector::TargetEngine;
        let profile = LaunchOverride {
            engine: Some("gptk".into()),
            ..Default::default()
        };
        let temporary = LaunchOverride {
            engine: Some("auto".into()),
            ..Default::default()
        };
        assert_eq!(
            requested_engine(
                Some(&temporary),
                "override.json",
                Some("typo"),
                &profile,
                "profile.json"
            )
            .unwrap(),
            Some(TargetEngine::Auto)
        );
        assert_eq!(
            requested_engine(
                None,
                "override.json",
                Some("auto"),
                &profile,
                "profile.json"
            )
            .unwrap(),
            Some(TargetEngine::Auto)
        );
        let error = requested_engine(
            None,
            "override.json",
            Some("dxmtt"),
            &profile,
            "profile.json",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("NUCLEON_ENGINE") && error.contains("dxmtt"));
        let bad = LaunchOverride {
            engine: Some("typo".into()),
            ..Default::default()
        };
        assert!(
            requested_engine(None, "override.json", None, &bad, "profile.json")
                .unwrap_err()
                .to_string()
                .contains("profile.json: engine")
        );
    }

    #[test]
    fn malformed_launch_settings_are_reported_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launch_override_123.json");
        for data in ["{", r#"{"hud":"yes"}"#, r#"{"engnie":"gptk"}"#] {
            fs::write(&path, data).unwrap();
            let error = format!("{:#}", load_launch_settings(&path).unwrap_err());
            assert!(
                error.contains("launch_override_123.json") && error.contains("Correct its JSON")
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), data);
        }
        fs::write(&path, r#"{"hud":true,"engine":"auto"}"#).unwrap();
        assert_eq!(
            load_launch_settings(&path).unwrap().unwrap().hud,
            Some(true)
        );
        assert!(
            load_launch_settings(&dir.path().join("missing.json"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_automatic_engine_preserves_backend_and_wine_overrides() {
        use nucleon_core::backend::GraphicsBackend;
        use nucleon_core::detector::{AutoBackends, GraphicsApi, TargetEngine};
        let available = AutoBackends {
            gptk: true,
            vkd3d: true,
            vulkan: true,
            ..AutoBackends::default()
        };
        assert_eq!(
            select_automatic_engine(
                &GraphicsApi::DirectX12,
                available,
                GraphicsBackend::Auto,
                false
            )
            .unwrap(),
            TargetEngine::Gptk
        );
        assert_eq!(
            select_automatic_engine(
                &GraphicsApi::DirectX12,
                available,
                GraphicsBackend::Auto,
                true
            )
            .unwrap(),
            TargetEngine::Vkd3d
        );
        for (backend, engine) in [
            (GraphicsBackend::D3DMetal, TargetEngine::Gptk),
            (GraphicsBackend::Dxmt, TargetEngine::Dxmt),
            (GraphicsBackend::Dxvk, TargetEngine::Dxvk),
            (GraphicsBackend::KosmicKrisp, TargetEngine::KosmicKrisp),
        ] {
            assert_eq!(
                select_automatic_engine(
                    &GraphicsApi::Unknown,
                    AutoBackends::default(),
                    backend,
                    true
                )
                .unwrap(),
                engine
            );
        }
    }

    #[test]
    fn test_is_wine_game_process_line_ignores_self_and_nucleon() {
        let my_pid = 85020;
        let line_self = "85020 ~/Library/Application Support/Steam/compatibilitytools.d/nucleon-wine/nucleon-runner waitforexitandrun ~/Library/Application Support/Steam/steamapps/common/DiRT Rally 2.0/dirtrally2.exe -novr";
        assert!(!is_wine_game_process_line(
            line_self,
            my_pid,
            "/wine",
            "dirtrally2.exe"
        ));

        // Even with a different PID, nucleon-runner itself must be ignored
        let line_other_runner = "16271 ~/Library/Application Support/Steam/compatibilitytools.d/nucleon/nucleon-runner waitforexitandrun ~/Library/Application Support/Steam/steamapps/common/DiRT Rally 2.0/dirtrally2.exe -novr";
        assert!(!is_wine_game_process_line(
            line_other_runner,
            my_pid,
            "/wine",
            "dirtrally2.exe"
        ));
    }

    #[test]
    fn test_is_wine_game_process_line_ignores_wine_services() {
        let my_pid = 99999;
        let line_services = "12345 /opt/wine/bin/wine64 C:\\windows\\system32\\services.exe";
        assert!(!is_wine_game_process_line(
            line_services,
            my_pid,
            "/opt/wine",
            "dirtrally2.exe"
        ));

        let line_winedevice = "12346 /opt/wine/bin/wine64 C:\\windows\\system32\\winedevice.exe";
        assert!(!is_wine_game_process_line(
            line_winedevice,
            my_pid,
            "/opt/wine",
            "dirtrally2.exe"
        ));

        let line_wineserver = "12347 /opt/wine/bin/wineserver -p";
        assert!(!is_wine_game_process_line(
            line_wineserver,
            my_pid,
            "/opt/wine",
            "dirtrally2.exe"
        ));

        let line_steam = "12348 /opt/wine/bin/wine64 C:\\Program Files (x86)\\Steam\\steam.exe";
        assert!(!is_wine_game_process_line(
            line_steam,
            my_pid,
            "/opt/wine",
            "dirtrally2.exe"
        ));

        let line_crashsender = "13004 /opt/wine/bin/wine-preloader Z:\\games\\DiRT Rally 2.0\\CrashSender1405.exe 0f736746";
        assert!(!is_wine_game_process_line(
            line_crashsender,
            my_pid,
            "/opt/wine",
            "dirtrally2.exe"
        ));
    }

    #[test]
    fn test_is_wine_game_process_line_detects_actual_game() {
        let my_pid = 99999;
        let line_game = "12348 /opt/wine/bin/wine64-preloader /opt/wine/bin/wine64 Z:\\games\\DiRT Rally 2.0\\dirtrally2.exe -novr";
        assert!(is_wine_game_process_line(
            line_game,
            my_pid,
            "/opt/wine",
            "dirtrally2.exe"
        ));

        let line_other_game = "12349 /opt/wine/bin/wine64 Z:\\games\\other\\launcher.exe";
        assert!(is_wine_game_process_line(
            line_other_game,
            my_pid,
            "/opt/wine",
            ""
        ));
    }

    #[test]
    fn test_regression_dirt2_64bit_steamworks_launches_directly() {
        // DiRT links Steamworks, but its validated 64-bit launch protocol is direct execution.
        let game_args = vec![
            "Z:\\games\\DiRT Rally 2.0\\dirtrally2.exe".to_string(),
            "-novr".to_string(),
        ];
        for engine in [
            nucleon_core::detector::TargetEngine::Gptk,
            nucleon_core::detector::TargetEngine::KosmicKrisp,
            nucleon_core::detector::TargetEngine::Dxmt,
        ] {
            let _ = engine; // the wrapper decision no longer reads the engine at all
            let wine_args = determine_wine_args(&game_args, false, true, None);
            assert_eq!(wine_args, game_args);
            assert!(!wine_args[0].contains("steam.exe"));
        }
    }

    #[test]
    fn test_regression_gonner_wine_combination_invokes_steam_shim() {
        // GoNNER scenario: 32-bit Unity title linked against Steamworks, steam.exe present in prefix
        let game_args = vec!["Z:\\games\\GoNNER\\GONNER.exe".to_string()];
        let wine_args = determine_wine_args(&game_args, true, true, None);

        // Must wrap through steam.exe so SteamAPI_Init handshake succeeds
        assert_eq!(wine_args.len(), 2);
        assert!(wine_args[0].contains("steam.exe"));
        assert_eq!(wine_args[1], "Z:\\games\\GoNNER\\GONNER.exe");

        // If steam_shim does NOT exist in prefix, falls back cleanly to direct execution
        let fallback_args = determine_wine_args(&game_args, true, false, None);
        assert_eq!(fallback_args, game_args);
    }

    #[test]
    fn test_dmc5_explicit_wrapper_override_is_engine_independent() {
        // DMC5 bridge experiments may explicitly request the wrapper without changing other games.
        let game_args = vec!["Z:\\games\\Devil May Cry 5\\DevilMayCry5.exe".to_string()];

        for engine in [
            nucleon_core::detector::TargetEngine::Gptk,
            nucleon_core::detector::TargetEngine::KosmicKrisp,
            nucleon_core::detector::TargetEngine::Dxvk,
            nucleon_core::detector::TargetEngine::Vkd3d,
        ] {
            let _ = engine;
            let wine_args = determine_wine_args(&game_args, false, true, Some(true));
            assert_eq!(
                wine_args.len(),
                2,
                "engine must not change the wrapper decision"
            );
            assert!(
                wine_args[0].contains("steam.exe"),
                "an explicit wrapper request must be honored on every engine"
            );
            assert_eq!(wine_args[1], game_args[0]);
        }
    }

    #[test]
    fn test_regression_user_override_precedence() {
        let game_args = vec!["Z:\\games\\test.exe".to_string()];

        // Explicit user override false forces direct execution even for a wrapped title
        let direct = determine_wine_args(&game_args, true, true, Some(false));
        assert_eq!(direct, game_args);

        // Explicit user override true forces the shim whenever the prefix has one
        let shimmed = determine_wine_args(&game_args, false, true, Some(true));
        assert_eq!(shimmed.len(), 2);
        assert!(shimmed[0].contains("steam.exe"));

        // An override cannot invent a shim the prefix does not have
        let no_shim = determine_wine_args(&game_args, true, false, Some(true));
        assert_eq!(no_shim, game_args);
    }

    #[test]
    fn test_golden_path_backend_staging_isolation() {
        use std::fs;
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        const CHILD_TEST: &str = "NUCLEON_BACKEND_STAGING_TEST_CHILD";
        if env::var_os(CHILD_TEST).is_none() {
            let output = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::test_golden_path_backend_staging_isolation",
                    "--nocapture",
                ])
                .env_clear()
                .env(CHILD_TEST, "1")
                .env("NUCLEON_SUPPORT_DIR", temp.path())
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && stdout.contains("1 passed;"),
                "Isolated backend staging test failed ({status}):\n{stdout}\n{stderr}",
                status = output.status,
                stderr = String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let pfx_dir = temp.path().join("pfx");
        let sys32 = pfx_dir.join("drive_c/windows/system32");
        let syswow64 = pfx_dir.join("drive_c/windows/syswow64");
        fs::create_dir_all(&sys32).unwrap();
        fs::create_dir_all(&syswow64).unwrap();

        // 1. Create mock DXVK bundle & register path
        let dxvk_dir = temp.path().join("dxvk");
        let dxvk_x64 = dxvk_dir.join("x64");
        fs::create_dir_all(&dxvk_x64).unwrap();
        fs::write(dxvk_x64.join("d3d11.dll"), b"dxvk-dll").unwrap();
        nucleon_core::dxvk::set_custom_dxvk_path(&dxvk_dir).unwrap();

        let dxvk_backend =
            nucleon_core::backend::create_backend(nucleon_core::backend::GraphicsBackend::Dxvk);
        let staged_dxvk = dxvk_backend.enable(&pfx_dir, None).unwrap();
        assert!(staged_dxvk > 0);
        assert_eq!(fs::read(sys32.join("d3d11.dll")).unwrap(), b"dxvk-dll");

        // 2. Create mock DXMT bundle & register path
        let dxmt_dir = temp.path().join("dxmt");
        let dxmt_x64 = dxmt_dir.join("x86_64-windows");
        fs::create_dir_all(&dxmt_x64).unwrap();
        fs::write(dxmt_x64.join("d3d11.dll"), b"dxmt-d3d11").unwrap();
        fs::write(dxmt_x64.join("winemetal.dll"), b"dxmt-winemetal").unwrap();
        nucleon_core::dxmt::set_custom_dxmt_path(&dxmt_dir).unwrap();

        let dxmt_backend =
            nucleon_core::backend::create_backend(nucleon_core::backend::GraphicsBackend::Dxmt);
        let staged_dxmt = dxmt_backend.enable(&pfx_dir, None).unwrap();
        assert!(staged_dxmt > 0);
        assert_eq!(fs::read(sys32.join("d3d11.dll")).unwrap(), b"dxmt-d3d11");
        assert_eq!(
            fs::read(sys32.join("winemetal.dll")).unwrap(),
            b"dxmt-winemetal"
        );

        // 3. Enable D3DMetal (GPTK4 flow) -> unstage all translation layers
        let d3dm_backend =
            nucleon_core::backend::create_backend(nucleon_core::backend::GraphicsBackend::D3DMetal);
        d3dm_backend.enable(&pfx_dir, None).unwrap();
        assert!(
            !sys32.join("winemetal.dll").exists(),
            "D3DMetal flow must unstage winemetal.dll"
        );
        assert!(
            !sys32.join("d3d11.dll").exists(),
            "D3DMetal flow must unstage translation d3d11.dll"
        );

        // Cleanup custom paths
        let _ = nucleon_core::dxvk::clear_custom_dxvk_path();
        let _ = nucleon_core::dxmt::clear_custom_dxmt_path();
    }
}
