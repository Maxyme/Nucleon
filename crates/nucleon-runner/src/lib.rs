#![forbid(unsafe_code)]

use anyhow::{bail, Result};
use nucleon_core::{paths, prefix, runner};
use serde::Deserialize;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

#[derive(Debug, Deserialize, Default)]
struct LaunchOverride {
    hud: Option<bool>,
    engine: Option<String>,
    use_steam_shim: Option<bool>,
}

/// Determines whether to wrap game arguments through Proton's `steam.exe` or execute directly.
///
/// Under GPTK (64-bit D3DMetal), direct execution is always used because 32-bit `steam.exe` wrapping
/// causes command-line and process spawning failures with 64-bit Direct3D titles.
/// Under Wine / backends, 32-bit legacy titles (such as GoNNER) are wrapped with `steam.exe` if present
/// in the prefix to satisfy the `SteamAPI_Init` client handshake, while 64-bit titles launch directly.
/// Explicit user overrides (via CLI `use_steam_shim` or `NUCLEON_STEAM_SHIM` env var) take precedence.
pub fn determine_wine_args(
    game_args: &[String],
    active_engine: nucleon_core::detector::TargetEngine,
    is_64_bit: bool,
    steam_shim_exists: bool,
    user_shim_override: Option<bool>,
) -> Vec<String> {
    let use_shim = match user_shim_override {
        Some(explicit) => explicit && steam_shim_exists,
        None => {
            if active_engine == nucleon_core::detector::TargetEngine::Gptk {
                false
            } else {
                !is_64_bit && steam_shim_exists
            }
        }
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
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("{ts} [nucleon-runner] {msg}\n");
    print!("{line}");
    let log_path = paths::support_dir().join("nucleon-runner.log");
    if let Some(p) = log_path.parent() {
        let _ = fs::create_dir_all(p);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn activate_frontmost_window() {
    let _ = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to set frontmost of first process whose bundle identifier is "com.valvesoftware.steam" to true"#,
        ])
        .status();
}

fn set_runner_lib_env(cmd: &mut Command, runner_dir: &Path) {
    let lib_dir = runner_dir.join("lib");
    let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
    if lib_dir.is_dir() || lib_unix.is_dir() {
        cmd.env(
            "DYLD_FALLBACK_LIBRARY_PATH",
            format!("{}:{}", lib_unix.display(), lib_dir.display()),
        );
    }
}

fn wait_wineserver(wineserver_bin: &Path, pfx_dir: &Path, timeout_ms: u64) {
    if !wineserver_bin.is_file() {
        return;
    }
    let mut cmd = Command::new(wineserver_bin);
    cmd.arg("-w");
    cmd.env("WINEPREFIX", pfx_dir);
    if let Some(runner_dir) = wineserver_bin.parent().and_then(|p| p.parent()) {
        set_runner_lib_env(&mut cmd, runner_dir);
    }

    if let Ok(mut child) = cmd.spawn() {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if start.elapsed() >= timeout {
                        let _ = child.kill();
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
    }
}

/// Recursively terminates all child processes of a given PID using sysinfo (kill-tree algorithm).
pub fn kill_process_tree_sysinfo(sys: &System, root_pid: Pid, signal: Signal) {
    let child_pids: Vec<Pid> = sys
        .processes()
        .iter()
        .filter(|(_, p)| p.parent() == Some(root_pid))
        .map(|(pid, _)| *pid)
        .collect();

    for child_pid in child_pids {
        kill_process_tree_sysinfo(sys, child_pid, signal);
    }

    if let Some(proc) = sys.process(root_pid) {
        let _ = proc.kill_with(signal);
    }
}

fn terminate_wine_prefix(
    runner_dir: &Path,
    pfx_dir: &Path,
    mut child: Option<&mut std::process::Child>,
    child_id: u32,
    target_exe: Option<&Path>,
) {
    let wineserver_bin = runner_dir.join("bin/wineserver");

    // 1. Tell wineserver to cleanly terminate all processes under this prefix with a bounded timeout
    if wineserver_bin.is_file() {
        let mut cmd = Command::new(&wineserver_bin);
        cmd.arg("-k");
        cmd.env("WINEPREFIX", pfx_dir);
        set_runner_lib_env(&mut cmd, runner_dir);
        if let Ok(mut c) = cmd.spawn() {
            let start = std::time::Instant::now();
            loop {
                if let Ok(Some(_)) = c.try_wait() {
                    break;
                }
                if start.elapsed() >= Duration::from_millis(1500) {
                    let _ = c.kill();
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let root_pid = Pid::from_u32(child_id);
    let runner_dir_str = runner_dir.to_string_lossy().to_lowercase();
    let runner_canon = runner_dir.canonicalize().unwrap_or_else(|_| runner_dir.to_path_buf());
    let runner_canon_str = runner_canon.to_string_lossy().to_lowercase();
    let pfx_str = pfx_dir.to_string_lossy().to_lowercase();
    let my_pid = std::process::id();
    let target_name = target_exe
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 2. Kill the primary child process tree recursively with SIGTERM (kill-tree)
    if child_id > 0 {
        kill_process_tree_sysinfo(&sys, root_pid, Signal::Term);
    }

    // 3. Scan via `ps -A -o pid,command` to find all processes (including Rosetta translated processes)
    let mut ps_pids = Vec::new();
    if let Ok(output) = Command::new("ps").args(["-A", "-o", "pid,command"]).output() {
        if let Ok(stdout) = String::from_utf8(output.stdout) {
            for line in stdout.lines() {
                let trimmed = line.trim();
                let mut parts = trimmed.split_whitespace();
                if let Some(pid_str) = parts.next() {
                    if let Ok(p) = pid_str.parse::<u32>() {
                        if p == my_pid || (child_id > 0 && p == child_id) {
                            continue;
                        }
                        let cmd_lower = trimmed[pid_str.len()..].trim().to_lowercase();
                        if cmd_lower.contains("nucleon-runner") || cmd_lower.contains("bin/nucleon") {
                            continue;
                        }
                        let matches_target = !target_name.is_empty() && cmd_lower.contains(&target_name);
                        let matches_runner = cmd_lower.contains(&runner_dir_str)
                            || cmd_lower.contains(&runner_canon_str)
                            || cmd_lower.contains(&pfx_str);
                        if matches_target || matches_runner {
                            ps_pids.push(p);
                        }
                    }
                }
            }
        }
    }

    for &p in &ps_pids {
        let _ = Command::new("kill").args(["-TERM", &p.to_string()]).status();
    }

    // 4. Terminate any sysinfo processes running under this Wine runner or prefix
    for (pid, proc) in sys.processes() {
        if pid.as_u32() == my_pid || (child_id > 0 && *pid == root_pid) {
            continue;
        }
        let exe_str = proc
            .exe()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let cmd_joined = proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();

        if cmd_joined.contains("nucleon-runner") || cmd_joined.contains("bin/nucleon") {
            continue;
        }

        if exe_str.contains(&runner_dir_str)
            || exe_str.contains(&runner_canon_str)
            || cmd_joined.contains(&runner_dir_str)
            || cmd_joined.contains(&runner_canon_str)
            || cmd_joined.contains(&pfx_str)
            || (!target_name.is_empty() && (exe_str.contains(&target_name) || cmd_joined.contains(&target_name)))
        {
            let _ = proc.kill_with(Signal::Term);
        }
    }

    // Brief grace period for processes to exit
    thread::sleep(Duration::from_millis(300));

    // Force SIGKILL on child if still around
    if let Some(ref mut c) = child {
        let _ = c.kill();
    }
    if child_id > 0 {
        sys.refresh_processes(ProcessesToUpdate::All, true);
        kill_process_tree_sysinfo(&sys, root_pid, Signal::Kill);
    }

    for &p in &ps_pids {
        let _ = Command::new("kill").args(["-KILL", &p.to_string()]).status();
    }

    for (pid, proc) in sys.processes() {
        if pid.as_u32() == my_pid || (child_id > 0 && *pid == root_pid) {
            continue;
        }
        let exe_str = proc
            .exe()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let cmd_joined = proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();

        if cmd_joined.contains("nucleon-runner") || cmd_joined.contains("bin/nucleon") {
            continue;
        }

        if exe_str.contains(&runner_dir_str)
            || exe_str.contains(&runner_canon_str)
            || cmd_joined.contains(&runner_dir_str)
            || cmd_joined.contains(&runner_canon_str)
            || cmd_joined.contains(&pfx_str)
            || (!target_name.is_empty() && (exe_str.contains(&target_name) || cmd_joined.contains(&target_name)))
        {
            let _ = proc.kill();
        }
    }

    // Wait briefly for wineserver socket cleanup (at most 1000ms)
    wait_wineserver(&wineserver_bin, pfx_dir, 1000);
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
    if let Ok(pid) = pid_str.parse::<u32>() {
        if pid == my_pid {
            return false;
        }
    }

    let args_part = trimmed[pid_str.len()..].trim().to_lowercase();

    // Ignore nucleon-runner and nucleon CLI
    if args_part.contains("nucleon-runner") || args_part.contains("bin/nucleon") {
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

pub fn is_wine_game_process_running(runner_dir: &Path, target_exe: &Path) -> bool {
    let my_pid = std::process::id();
    let runner_dir_str = runner_dir.to_string_lossy().to_lowercase();
    let target_name = target_exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 1. Check via `ps -A -o pid,command` which reliably captures arguments of translated Rosetta processes on macOS
    if let Ok(output) = Command::new("ps")
        .args(["-A", "-o", "pid,command"])
        .output()
    {
        if let Ok(stdout) = String::from_utf8(output.stdout) {
            for line in stdout.lines() {
                if line.contains("wine") || line.contains(&target_name) {
                    log_runner(&format!("PS LINE: {}", line));
                }
                if is_wine_game_process_line(line, my_pid, &runner_dir_str, &target_name) {
                    log_runner(&format!("MATCHED GAME: {}", line));
                    return true;
                }
            }
        }
    }

    // 2. Fallback to sysinfo in case ps is unavailable
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    for (pid, proc) in sys.processes() {
        if pid.as_u32() == my_pid {
            continue;
        }
        let exe_str = proc
            .exe()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let cmd_joined = proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let line = format!("{} {} {}", pid.as_u32(), exe_str, cmd_joined);

        if is_wine_game_process_line(&line, my_pid, &runner_dir_str, &target_name) {
            return true;
        }
    }
    false
}

pub fn run_with_args(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        eprintln!("Usage: nucleon-runner <verb> [args...]");
        std::process::exit(1);
    }

    let verb = &args[1];

    if verb == "check-app-compatibility" {
        std::process::exit(0);
    }

    // Bypass iscriptevaluator.exe immediately
    for arg in args {
        if arg.contains("iscriptevaluator.exe") {
            log_runner("Intercepted iscriptevaluator.exe, exiting cleanly");
            std::process::exit(0);
        }
    }

    // Parse Steam compatibility environment
    let compat_appid = env::var("STEAM_COMPAT_APP_ID")
        .ok()
        .or_else(|| env::var("SteamAppId").ok())
        .or_else(|| {
            env::var_os("STEAM_COMPAT_DATA_PATH").and_then(|p| {
                let path = PathBuf::from(p);
                path.file_name()
                    .and_then(|n| n.to_str())
                    .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
                    .map(|s| s.to_string())
            })
        });
    let compat_data_path = env::var_os("STEAM_COMPAT_DATA_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::support_dir().join("default_prefix"));
    let pfx_dir = compat_data_path.join("pfx");
    env::set_var("WINEPREFIX", &pfx_dir);

    // Determine target executable and arguments
    // Steam invokes: nucleon-runner run <exe_path> [game_args...]
    let game_args: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        vec![]
    };

    if game_args.is_empty() {
        log_runner("No game executable specified to run, exiting.");
        std::process::exit(0);
    }

    let target_exe = PathBuf::from(&game_args[0]);

    if let Some(ref id) = compat_appid {
        env::set_var("SteamAppId", id);
        env::set_var("SteamGameId", id);
        if let Some(target_dir) = target_exe.parent() {
            let appid_path = target_dir.join("steam_appid.txt");
            if !appid_path.exists() {
                let _ = fs::write(&appid_path, id);
            }
        }
    }

    // Check for launch overrides (from CLI launch command)
    let mut launch_override: Option<LaunchOverride> = None;
    if let Some(ref id) = compat_appid {
        let override_file = paths::support_dir().join(format!("launch_override_{id}.json"));
        if override_file.is_file() {
            if let Ok(data) = fs::read_to_string(&override_file) {
                if let Ok(ov) = serde_json::from_str::<LaunchOverride>(&data) {
                    launch_override = Some(ov);
                }
            }
            let _ = fs::remove_file(&override_file);
        }
    }

    // Resolve requested engine override
    let requested_engine = launch_override
        .as_ref()
        .and_then(|o| o.engine.clone())
        .or_else(|| env::var("NUCLEON_ENGINE").ok())
        .and_then(|s| nucleon_core::detector::TargetEngine::parse(&s));

    let (engine, api_desc) = match requested_engine {
        Some(nucleon_core::detector::TargetEngine::Auto) | None => {
            // Auto runner is for Wine only: automatically selects the optimal Wine graphics backend
            // (Mesa KosmicKrisp Vulkan for DX11/12/Vulkan/D7VK/OpenGL via Zink vs WineD3D for DX9/10).
            let detection = nucleon_core::detector::detect_target_engine(&target_exe);
            let wine_engine = detection.wine_engine();
            log_runner(&format!(
                "Wine Auto Runner: detected graphics API {:?} (found: {:?}) -> routing within Wine to {:?}",
                detection.api, detection.detected_dll, wine_engine
            ));
            (
                wine_engine,
                format!(
                    "Wine Auto Detection: {:?} (DLL: {:?}) -> {}",
                    detection.api,
                    detection.detected_dll,
                    wine_engine.display_name()
                ),
            )
        }
        Some(eng) => {
            log_runner(&format!("Engine manually overridden -> {:?}", eng));
            (eng, format!("Manual Override ({:?})", eng))
        }
    };

    log_runner(&format!(
        "Engine Router: target '{}' [{}] -> engine {:?}",
        target_exe.display(),
        api_desc,
        engine
    ));

    // Resolve optimal runner for selected engine
    let (mut runner_dir, active_engine) = runner::resolve_runner_for_engine(engine)?;

    // If running under Wine (WineStaging, KosmicKrisp, Dxvk, Vkd3d, or Auto), allow per-game NUCLEON_WINE launch override or NUCLEON_WINE_PATH
    if active_engine == nucleon_core::detector::TargetEngine::WineStaging
        || active_engine == nucleon_core::detector::TargetEngine::KosmicKrisp
        || active_engine == nucleon_core::detector::TargetEngine::Dxvk
        || active_engine == nucleon_core::detector::TargetEngine::Vkd3d
        || active_engine == nucleon_core::detector::TargetEngine::Dxmt
        || active_engine == nucleon_core::detector::TargetEngine::Auto
    {
        if let Ok(wine_override) = env::var("NUCLEON_WINE") {
            if let Some(resolved) =
                nucleon_core::wine::resolve_wine_runtime_by_query(&wine_override)
            {
                log_runner(&format!(
                    "Per-game NUCLEON_WINE override matched: '{}' -> {}",
                    resolved.name,
                    resolved.root.display()
                ));
                runner_dir = resolved.root;
            } else {
                log_runner(&format!("Per-game NUCLEON_WINE='{}' could not be resolved, falling back to default runner", wine_override));
            }
        } else if let Ok(custom_wine) =
            env::var("NUCLEON_WINE_PATH").or_else(|_| env::var("WINE_PATH"))
        {
            if let Some(resolved) = nucleon_core::wine::resolve_wine_runtime_by_query(&custom_wine)
            {
                log_runner(&format!(
                    "Per-game NUCLEON_WINE_PATH override matched: '{}' -> {}",
                    resolved.name,
                    resolved.root.display()
                ));
                runner_dir = resolved.root;
            } else {
                let wp = PathBuf::from(&custom_wine);
                if wp.join("bin/wine").is_file() || wp.join("bin/wine64").is_file() {
                    runner_dir = wp;
                }
            }
        }
    }

    let wine_bin = if runner_dir.join("bin/wine").is_file() {
        runner_dir.join("bin/wine")
    } else {
        runner_dir.join("bin/wine64")
    };
    let wineserver_bin = runner_dir.join("bin/wineserver");

    if !wine_bin.exists() {
        bail!("Wine binary not found at {}", wine_bin.display());
    }

    // Wait for previous instance if requested
    if verb == "waitforexitandrun" {
        wait_wineserver(&wineserver_bin, &pfx_dir, 5000);
    }

    // Initialize prefix, registry, and bridge DLLs
    prefix::ensure_prefix(&pfx_dir, &runner_dir)?;

    // Stage or unstage translation DLLs based on active backend:
    let backend_choice = match active_engine {
        nucleon_core::detector::TargetEngine::Gptk => nucleon_core::backend::GraphicsBackend::D3DMetal,
        nucleon_core::detector::TargetEngine::Dxmt => nucleon_core::backend::GraphicsBackend::Dxmt,
        nucleon_core::detector::TargetEngine::Dxvk => nucleon_core::backend::GraphicsBackend::Dxvk,
        nucleon_core::detector::TargetEngine::KosmicKrisp | nucleon_core::detector::TargetEngine::Vkd3d => {
            nucleon_core::backend::GraphicsBackend::KosmicKrisp
        }
        nucleon_core::detector::TargetEngine::WineStaging => nucleon_core::backend::GraphicsBackend::Auto,
        nucleon_core::detector::TargetEngine::Auto => {
            let configured = nucleon_core::backend::get_active_backend();
            if configured != nucleon_core::backend::GraphicsBackend::Auto {
                configured
            } else {
                nucleon_core::backend::GraphicsBackend::Auto
            }
        }
    };

    let backend = nucleon_core::backend::create_backend(backend_choice);
    match backend_choice {
        nucleon_core::backend::GraphicsBackend::Auto => {
            let _ = backend.disable(&pfx_dir, Some(&runner_dir));
        }
        nucleon_core::backend::GraphicsBackend::D3DMetal => {
            let _ = backend.enable(&pfx_dir, Some(&runner_dir));
        }
        _ => {
            match backend.enable(&pfx_dir, Some(&runner_dir)) {
                Ok(staged) => {
                    log_runner(&format!(
                        "{} active ({} DLL(s) staged into prefix)",
                        backend.display_name(),
                        staged
                    ));
                }
                Err(e) => {
                    log_runner(&format!(
                        "WARNING: Failed to stage backend {}: {:#}",
                        backend.display_name(),
                        e
                    ));
                }
            }
        }
    }

    let enable_hud = launch_override
        .as_ref()
        .and_then(|o| o.hud)
        .unwrap_or_else(|| {
            env::var("MTL_HUD_ENABLED")
                .map(|v| v == "1")
                .unwrap_or(false)
        });

    if enable_hud {
        log_runner("Metal Performance HUD enabled");
    }

    let mut exec_env =
        runner::build_execution_env_for_engine(&runner_dir, &pfx_dir, active_engine, enable_hud);

    if let Some(ref id) = compat_appid {
        exec_env.insert("SteamAppId".to_string(), id.clone());
        exec_env.insert("SteamGameId".to_string(), id.clone());
    }

    // Setup signal handler for prompt SIGTERM exit
    let term_flag = Arc::new(AtomicBool::new(false));
    let term_clone = Arc::clone(&term_flag);

    ctrlc::set_handler(move || {
        term_clone.store(true, Ordering::SeqCst);
    })
    .ok();

    // Working directory is the parent of the exe if available
    let game_cwd = if let Some(exe_path) = game_args.first().map(Path::new) {
        if let Some(parent) = exe_path.parent() {
            if parent.is_dir() {
                Some(parent.to_path_buf())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let steam_shim = pfx_dir.join("drive_c/Program Files (x86)/Steam/steam.exe");
    let is_64 = nucleon_core::detector::is_pe_file_64_bit(&target_exe);
    let user_shim_override = launch_override
        .as_ref()
        .and_then(|o| o.use_steam_shim)
        .or_else(|| {
            env::var("NUCLEON_STEAM_SHIM").ok().map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        });

    let wine_args = determine_wine_args(
        &game_args,
        active_engine,
        is_64,
        steam_shim.is_file(),
        user_shim_override,
    );

    log_runner(&format!("Wine binary: {}", wine_bin.display()));
    log_runner(&format!("Game CWD: {:?}", game_cwd));
    log_runner(&format!("Target architecture: {}", if is_64 { "64-bit" } else { "32-bit" }));
    log_runner(&format!("Launching game with Wine: {:?}", wine_args));

    for (k, v) in env::vars() {
        log_runner(&format!("INHERITED_ENV: {}={}", k, v));
    }

    let mut cmd = Command::new(&wine_bin);
    cmd.args(&wine_args);

    for (k, v) in &exec_env {
        log_runner(&format!("ENV: {}={}", k, v));
        cmd.env(k, v);
    }

    if let Some(ref cwd) = game_cwd {
        cmd.current_dir(cwd);
    }

    let wine_log_path = paths::support_dir().join("wine.log");
    if let Ok(file) = fs::File::create(&wine_log_path) {
        if let Ok(err_file) = file.try_clone() {
            cmd.stdout(std::process::Stdio::from(file));
            cmd.stderr(std::process::Stdio::from(err_file));
        }
    }

    let mut child = cmd.spawn()?;

    // Focus activation after slight delay
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(2500));
        activate_frontmost_window();
    });

    // Supervision watchdog
    let mut primary_exited = false;
    let mut exit_code = 0;

    loop {
        if term_flag.load(Ordering::SeqCst) {
            log_runner("Termination signal received, stopping Wine processes cleanly");
            let child_id = child.id();
            terminate_wine_prefix(&runner_dir, &pfx_dir, Some(&mut child), child_id, Some(&target_exe));
            std::process::exit(0);
        }

        if !primary_exited {
            match child.try_wait() {
                Ok(Some(status)) => {
                    primary_exited = true;
                    exit_code = status.code().unwrap_or(0);
                    log_runner(&format!("Wine primary process exited with: {:?}", status));

                    // When Wine acts as a launcher that spawns/forks wine-preloader and exits,
                    // poll for up to 5 seconds to detect the background Wine game process before deciding to exit.
                    let mut found_game = false;
                    for _ in 0..25 {
                        if is_wine_game_process_running(&runner_dir, &target_exe) {
                            found_game = true;
                            break;
                        }
                        thread::sleep(Duration::from_millis(200));
                    }

                    if found_game {
                        log_runner("Detected active Wine game process; continuing supervision");
                        exit_code = 0;
                    } else {
                        log_runner("No Wine game processes detected after primary launcher exit, shutting down");
                        terminate_wine_prefix(&runner_dir, &pfx_dir, None, 0, Some(&target_exe));
                        break;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    log_runner(&format!("Error waiting on Wine process: {}", e));
                    exit_code = 1;
                    terminate_wine_prefix(&runner_dir, &pfx_dir, None, 0, Some(&target_exe));
                    break;
                }
            }
        } else {
            // Check if any background Wine game processes are still running.
            // Require two consecutive negative checks (500ms apart) to avoid false positives during process state changes.
            if !is_wine_game_process_running(&runner_dir, &target_exe) {
                thread::sleep(Duration::from_millis(500));
                if !is_wine_game_process_running(&runner_dir, &target_exe) {
                    log_runner("All Wine game processes have terminated, exiting cleanly");
                    terminate_wine_prefix(&runner_dir, &pfx_dir, None, 0, Some(&target_exe));
                    break;
                }
            }
        }

        thread::sleep(Duration::from_millis(500));
    }

    std::process::exit(exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_kill_process_tree_sysinfo_terminates_process() {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("Failed to spawn sleep command");

        let pid = Pid::from_u32(child.id());
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::All, true);
        assert!(sys.process(pid).is_some());

        kill_process_tree_sysinfo(&sys, pid, Signal::Term);

        // Wait up to 1 second for child to terminate
        let mut exited = false;
        for _ in 0..10 {
            if let Ok(Some(_)) = child.try_wait() {
                exited = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        if !exited {
            let _ = child.kill();
        }
        assert!(
            exited,
            "Process should have been terminated by kill_process_tree_sysinfo"
        );
    }

    #[test]
    fn test_regression_dirt2_gptk_combination_invokes_direct_execution() {
        // DiRT Rally 2.0 scenario: 64-bit Direct3D 11 game running via GPTK
        let game_args = vec![
            "Z:\\games\\DiRT Rally 2.0\\dirtrally2.exe".to_string(),
            "-novr".to_string(),
        ];
        let wine_args = determine_wine_args(
            &game_args,
            nucleon_core::detector::TargetEngine::Gptk,
            true, // is_64_bit = true
            true, // steam_shim_exists = true in prefix
            None, // no explicit user override
        );

        // Must launch directly without wrapping through Proton's 32-bit steam.exe
        assert_eq!(wine_args, game_args);
        assert!(!wine_args[0].contains("steam.exe"));
    }

    #[test]
    fn test_regression_gonner_wine_combination_invokes_steam_shim() {
        // GoNNER scenario: 32-bit Unity game running via Wine-Staging with steam.exe in prefix
        let game_args = vec!["Z:\\games\\GoNNER\\GONNER.exe".to_string()];
        let wine_args = determine_wine_args(
            &game_args,
            nucleon_core::detector::TargetEngine::WineStaging,
            false, // is_64_bit = false (32-bit game)
            true,  // steam_shim_exists = true
            None,  // no explicit user override
        );

        // Must wrap through steam.exe so SteamAPI_Init handshake succeeds
        assert_eq!(wine_args.len(), 2);
        assert!(wine_args[0].contains("steam.exe"));
        assert_eq!(wine_args[1], "Z:\\games\\GoNNER\\GONNER.exe");

        // If steam_shim does NOT exist in prefix, falls back cleanly to direct execution
        let fallback_args = determine_wine_args(
            &game_args,
            nucleon_core::detector::TargetEngine::WineStaging,
            false,
            false, // steam_shim_exists = false
            None,
        );
        assert_eq!(fallback_args, game_args);
    }

    #[test]
    fn test_regression_wine_dxmt_64bit_combination_invokes_direct_execution() {
        // 64-bit Direct3D 11 game running via DXMT
        let game_args = vec!["Z:\\games\\Game\\game.exe".to_string()];
        let wine_args = determine_wine_args(
            &game_args,
            nucleon_core::detector::TargetEngine::Dxmt,
            true, // is_64_bit = true
            true, // steam_shim_exists = true
            None,
        );
        assert_eq!(wine_args, game_args);
    }

    #[test]
    fn test_regression_user_override_precedence() {
        let game_args = vec!["Z:\\games\\test.exe".to_string()];

        // Explicit user override false forces direct execution even for 32-bit Wine title
        let direct = determine_wine_args(
            &game_args,
            nucleon_core::detector::TargetEngine::WineStaging,
            false,
            true,
            Some(false),
        );
        assert_eq!(direct, game_args);

        // Explicit user override true forces steam shim even for 64-bit GPTK title if shim exists
        let shimmed = determine_wine_args(
            &game_args,
            nucleon_core::detector::TargetEngine::Gptk,
            true,
            true,
            Some(true),
        );
        assert_eq!(shimmed.len(), 2);
        assert!(shimmed[0].contains("steam.exe"));
    }

    #[test]
    fn test_golden_path_backend_staging_isolation() {
        use std::fs;
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
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

        let dxvk_backend = nucleon_core::backend::create_backend(nucleon_core::backend::GraphicsBackend::Dxvk);
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

        let dxmt_backend = nucleon_core::backend::create_backend(nucleon_core::backend::GraphicsBackend::Dxmt);
        let staged_dxmt = dxmt_backend.enable(&pfx_dir, None).unwrap();
        assert!(staged_dxmt > 0);
        assert_eq!(fs::read(sys32.join("d3d11.dll")).unwrap(), b"dxmt-d3d11");
        assert_eq!(fs::read(sys32.join("winemetal.dll")).unwrap(), b"dxmt-winemetal");

        // 3. Enable D3DMetal (GPTK4 flow) -> unstage all translation layers
        let d3dm_backend = nucleon_core::backend::create_backend(nucleon_core::backend::GraphicsBackend::D3DMetal);
        d3dm_backend.enable(&pfx_dir, None).unwrap();
        assert!(!sys32.join("winemetal.dll").exists(), "D3DMetal flow must unstage winemetal.dll");
        assert!(!sys32.join("d3d11.dll").exists(), "D3DMetal flow must unstage translation d3d11.dll");

        // Cleanup custom paths
        let _ = nucleon_core::dxvk::clear_custom_dxvk_path();
        let _ = nucleon_core::dxmt::clear_custom_dxmt_path();
    }
}
