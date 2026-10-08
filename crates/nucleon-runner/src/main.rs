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

#[derive(Debug, Deserialize)]
struct LaunchOverride {
    hud: Option<bool>,
    engine: Option<String>,
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

fn terminate_wine_prefix(runner_dir: &Path, pfx_dir: &Path, child_id: u32) {
    let wineserver_bin = runner_dir.join("bin/wineserver");

    // 1. Tell wineserver to cleanly terminate all processes under this prefix
    if wineserver_bin.is_file() {
        let mut cmd = Command::new(&wineserver_bin);
        cmd.arg("-k");
        cmd.env("WINEPREFIX", pfx_dir);
        set_runner_lib_env(&mut cmd, runner_dir);
        let _ = cmd.status();
    }

    // 2. Send SIGTERM to the primary child process if still alive
    unsafe {
        libc::kill(child_id as i32, libc::SIGTERM);
    }

    // 3. Find and terminate any remaining processes associated with this runner or prefix
    let runner_dir_str = runner_dir.to_string_lossy().to_lowercase();
    let pfx_str = pfx_dir.to_string_lossy().to_lowercase();
    let my_pid = std::process::id();

    if let Ok(out) = Command::new("ps").args(["-ww", "-eo", "pid,args"]).output() {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let trimmed = line.trim();
            let mut parts = trimmed.split_whitespace();
            if let Some(pid_str) = parts.next() {
                if let Ok(pid) = pid_str.parse::<i32>() {
                    if pid as u32 == my_pid {
                        continue;
                    }
                    let lower = trimmed.to_lowercase();
                    if lower.contains("nucleon-runner") || lower.contains("bin/nucleon") {
                        continue;
                    }
                    if lower.contains(&runner_dir_str) || lower.contains(&pfx_str) {
                        unsafe {
                            libc::kill(pid, libc::SIGTERM);
                        }
                    }
                }
            }
        }
    }

    // Brief grace period for processes to exit
    thread::sleep(Duration::from_millis(300));

    // Force SIGKILL on child if still around
    unsafe {
        libc::kill(child_id as i32, libc::SIGKILL);
    }

    // Wait briefly for wineserver socket cleanup (at most 1000ms)
    wait_wineserver(&wineserver_bin, pfx_dir, 1000);
}

fn is_wine_game_process_line(
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

    // Exclude Wine infrastructure and services
    if args_part.contains("wineserver")
        || args_part.contains("winedevice.exe")
        || args_part.contains("services.exe")
        || args_part.contains("plugplay.exe")
        || args_part.contains("svchost.exe")
        || args_part.contains("rpcss.exe")
        || args_part.contains("explorer.exe")
        || args_part.contains("conhost.exe")
    {
        return false;
    }

    let matches_runner = args_part.contains(runner_dir_str)
        || args_part.contains("wine64-preloader")
        || args_part.contains("wine-preloader")
        || args_part.contains("wine64")
        || args_part.contains("/wine");

    let matches_target = !target_name.is_empty() && args_part.contains(target_name);

    (matches_runner || matches_target) && args_part.contains(".exe")
}

fn is_wine_game_process_running(runner_dir: &Path, target_exe: &Path) -> bool {
    let my_pid = std::process::id();
    let target_name = target_exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    let runner_dir_str = runner_dir.to_string_lossy().to_lowercase();

    let output = Command::new("ps").args(["-ww", "-eo", "pid,args"]).output();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if is_wine_game_process_line(line, my_pid, &runner_dir_str, &target_name) {
                return true;
            }
        }
    }
    false
}

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: nucleon-runner <verb> [args...]");
        std::process::exit(1);
    }

    let verb = &args[1];

    if verb == "check-app-compatibility" {
        std::process::exit(0);
    }

    // Bypass iscriptevaluator.exe immediately
    for arg in &args {
        if arg.contains("iscriptevaluator.exe") {
            log_runner("Intercepted iscriptevaluator.exe, exiting cleanly");
            std::process::exit(0);
        }
    }

    // Parse Steam compatibility environment
    let compat_appid = env::var("STEAM_COMPAT_APP_ID").ok();
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

    let (engine, api_desc) = if let Some(eng) = requested_engine {
        log_runner(&format!("Engine manually overridden -> {:?}", eng));
        (eng, format!("Manual Override ({:?})", eng))
    } else {
        let detection = nucleon_core::detector::detect_target_engine(&target_exe);
        log_runner(&format!(
            "Auto-detected graphics API: {:?} (found: {:?}) -> routing to {:?}",
            detection.api, detection.detected_dll, detection.engine
        ));
        (
            detection.engine,
            format!("{:?} (DLL: {:?})", detection.api, detection.detected_dll),
        )
    };

    log_runner(&format!(
        "Engine Router: target '{}' [{}] -> engine {:?}",
        target_exe.display(),
        api_desc,
        engine
    ));

    // Resolve optimal runner for selected engine
    let (mut runner_dir, active_engine) = runner::resolve_runner_for_engine(engine)?;

    // If running under WineStaging, allow per-game NUCLEON_WINE launch override or NUCLEON_WINE_PATH
    if active_engine == nucleon_core::detector::TargetEngine::WineStaging {
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
        } else if let Ok(custom_wine) = env::var("NUCLEON_WINE_PATH") {
            let wp = PathBuf::from(custom_wine);
            if wp.join("bin/wine").is_file() {
                runner_dir = wp;
            }
        }
    }

    let wine_bin = runner_dir.join("bin/wine");
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

    // Stage or unstage translation DLLs based on active engine:
    // KosmicKrisp: stages VKD3D-Proton (Direct3D 12 -> Vulkan 1.4) and D7VK (DirectDraw / D3D 1-7 -> Vulkan 1.4)
    // Other engines (e.g. GPTK): unstages VKD3D-Proton and D7VK to use native D3DMetal/WineD3D without DLL override conflicts
    if active_engine == nucleon_core::detector::TargetEngine::KosmicKrisp {
        if let Some(vkd3d) = nucleon_core::vkd3d::find_vkd3d_proton() {
            if let Ok(staged) =
                nucleon_core::vkd3d::stage_vkd3d_proton_into_prefix(&vkd3d, &pfx_dir)
            {
                log_runner(&format!(
                    "VKD3D-Proton active ({} DLL(s) from {}): Direct3D 12 -> Vulkan 1.4 -> KosmicKrisp",
                    staged,
                    vkd3d.root.display()
                ));
            }
        } else {
            log_runner(
                "VKD3D-Proton not installed. Point to an extracted path via 'nucleon vkd3d set-path <DIR>' or set VKD3D_PROTON_PATH to enable Direct3D 12 on KosmicKrisp."
            );
        }

        if let Some(d7vk) = nucleon_core::d7vk::find_d7vk() {
            if let Ok(staged) = nucleon_core::d7vk::stage_d7vk_into_prefix(&d7vk, &pfx_dir) {
                log_runner(&format!(
                    "D7VK active ({} DLL(s) from {}): DirectDraw / Direct3D 1-7 -> Vulkan 1.4 -> KosmicKrisp",
                    staged,
                    d7vk.root.display()
                ));
            }
        }
    } else {
        if let Ok(removed) =
            nucleon_core::vkd3d::unstage_vkd3d_proton_from_prefix(&pfx_dir, Some(&runner_dir))
        {
            if removed > 0 {
                log_runner(&format!(
                    "Unstaged {} VKD3D-Proton DLL(s) from prefix (restored builtin D3D12 for {:?})",
                    removed, active_engine
                ));
            }
        }
        if let Ok(removed) =
            nucleon_core::d7vk::unstage_d7vk_from_prefix(&pfx_dir, Some(&runner_dir))
        {
            if removed > 0 {
                log_runner(&format!(
                    "Unstaged {} D7VK DLL(s) from prefix (restored builtin ddraw for {:?})",
                    removed, active_engine
                ));
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

    let exec_env =
        runner::build_execution_env_for_engine(&runner_dir, &pfx_dir, active_engine, enable_hud);

    // Setup signal handler for prompt SIGTERM exit
    let term_flag = Arc::new(AtomicBool::new(false));
    let term_clone = Arc::clone(&term_flag);

    ctrlc::set_handler(move || {
        term_clone.store(true, Ordering::SeqCst);
    })
    .ok();

    log_runner(&format!("Launching game with Wine: {:?}", game_args));

    let mut cmd = Command::new(&wine_bin);
    cmd.args(&game_args);

    for (k, v) in exec_env {
        cmd.env(k, v);
    }

    // Working directory is the parent of the exe if available
    if let Some(exe_path) = game_args.first().map(Path::new) {
        if let Some(parent) = exe_path.parent() {
            if parent.is_dir() {
                cmd.current_dir(parent);
            }
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
            terminate_wine_prefix(&runner_dir, &pfx_dir, child.id());
            std::process::exit(0);
        }

        if !primary_exited {
            match child.try_wait() {
                Ok(Some(status)) => {
                    primary_exited = true;
                    exit_code = status.code().unwrap_or(0);
                    log_runner(&format!("Wine primary process exited with: {:?}", status));
                    if !is_wine_game_process_running(&runner_dir, &target_exe) {
                        wait_wineserver(&wineserver_bin, &pfx_dir, 2000);
                        break;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    log_runner(&format!("Error waiting on Wine process: {}", e));
                    exit_code = 1;
                    break;
                }
            }
        } else {
            // Check if any background Wine game processes are still running
            if !is_wine_game_process_running(&runner_dir, &target_exe) {
                log_runner("All Wine game processes have terminated, exiting cleanly");
                wait_wineserver(&wineserver_bin, &pfx_dir, 2000);
                break;
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
        let line_self = "85020 /Users/testuser/Library/Application Support/Steam/compatibilitytools.d/nucleon-wine/nucleon-runner waitforexitandrun /Users/testuser/Library/Application Support/Steam/steamapps/common/DiRT Rally 2.0/dirtrally2.exe -novr";
        assert!(!is_wine_game_process_line(
            line_self,
            my_pid,
            "/wine",
            "dirtrally2.exe"
        ));

        // Even with a different PID, nucleon-runner itself must be ignored
        let line_other_runner = "16271 /Users/testuser/Library/Application Support/Steam/compatibilitytools.d/nucleon/nucleon-runner waitforexitandrun /Users/testuser/Library/Application Support/Steam/steamapps/common/DiRT Rally 2.0/dirtrally2.exe -novr";
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
}
