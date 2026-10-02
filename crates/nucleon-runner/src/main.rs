use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use anyhow::{bail, Result};
use nucleon_core::{paths, prefix, runner};

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
    let output = Command::new("ps")
        .args(["-ww", "-eo", "pid,args"])
        .output();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let lower = line.to_lowercase();
            if lower.contains("wine") && lower.contains(".exe") {
                if !lower.contains("winedevice.exe")
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
            log::info!("Intercepted iscriptevaluator.exe, exiting cleanly");
            std::process::exit(0);
        }
    }

    // Parse Steam compatibility environment
    let compat_data_path = env::var_os("STEAM_COMPAT_DATA_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::support_dir().join("default_prefix"));
    let pfx_dir = compat_data_path.join("pfx");

    // Resolve runner
    let runner_dir = if paths::current_runner().exists() {
        paths::current_runner()
    } else {
        runner::assemble_runner(false)?
    };

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

    // Determine target executable and arguments
    // Steam invokes: nucleon-runner run <exe_path> [game_args...]
    let game_args: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        vec![]
    };

    if game_args.is_empty() {
        eprintln!("No game executable specified to run.");
        std::process::exit(0);
    }

    let enable_hud = env::var("MTL_HUD_ENABLED").map(|v| v == "1").unwrap_or(false);
    let exec_env = runner::build_execution_env(&runner_dir, &pfx_dir, enable_hud);

    // Setup signal handler for prompt SIGTERM exit
    let term_flag = Arc::new(AtomicBool::new(false));
    let term_clone = Arc::clone(&term_flag);

    ctrlc::set_handler(move || {
        term_clone.store(true, Ordering::SeqCst);
    }).ok();

    log::info!("Launching game with Wine: {:?}", game_args);

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
            log::info!("SIGTERM received, killing Wine processes cleanly");
            let _ = child.kill();
            let _ = Command::new(&wineserver_bin).arg("-k").status();
            let _ = Command::new(&wineserver_bin).arg("-w").status();
            std::process::exit(0);
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                log::info!("Wine primary process exited with: {:?}", status);
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
                eprintln!("Error waiting on Wine process: {}", e);
                break;
            }
        }
    }

    Ok(())
}
