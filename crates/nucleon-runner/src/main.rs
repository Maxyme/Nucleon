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

fn current_timestamp() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let mut buf = [0u8; 64];
        let len = libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            c"%Y-%m-%d %H:%M:%S".as_ptr(),
            &tm,
        );
        if len > 0 {
            String::from_utf8_lossy(&buf[..len as usize]).to_string()
        } else {
            String::new()
        }
    }
}

fn log_runner(msg: &str) {
    let ts = current_timestamp();
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

fn is_wine_game_process_running(_runner_dir: &Path) -> bool {
    // Check processes for running .exe binaries excluding wineserver/services
    let output = Command::new("ps").args(["-ww", "-eo", "pid,args"]).output();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let lower = line.to_lowercase();
            if lower.contains("wine")
                && lower.contains(".exe")
                && !lower.contains("winedevice.exe")
                && !lower.contains("services.exe")
                && !lower.contains("plugplay.exe")
                && !lower.contains("svchost.exe")
                && !lower.contains("rpcss.exe")
                && !lower.contains("explorer.exe")
            {
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
        let _ = Command::new(&wineserver_bin).arg("-w").status();
    }

    // Initialize prefix, registry, and bridge DLLs
    prefix::ensure_prefix(&pfx_dir, &runner_dir)?;

    // Stage VKD3D-Proton for KosmicKrisp Direct3D 12 translation
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
    loop {
        if term_flag.load(Ordering::SeqCst) {
            log_runner("SIGTERM received, killing Wine processes cleanly");
            let _ = child.kill();
            let _ = Command::new(&wineserver_bin).arg("-k").status();
            let _ = Command::new(&wineserver_bin).arg("-w").status();
            std::process::exit(0);
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                log_runner(&format!("Wine primary process exited with: {:?}", status));
                // Check if any background Wine game processes are still running
                if !is_wine_game_process_running(&runner_dir) {
                    let _ = Command::new(&wineserver_bin).arg("-w").status();
                    std::process::exit(status.code().unwrap_or(0));
                }
            }
            Ok(None) => {
                thread::sleep(Duration::from_millis(500));
            }
            Err(e) => {
                log_runner(&format!("Error waiting on Wine process: {}", e));
                break;
            }
        }
    }

    Ok(())
}
