use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::{guard, paths};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Subcommand, Debug, Clone)]
pub enum GuardAction {
    /// Inspect status of codesigning, WebUI patches, and LaunchAgent
    Status,
    /// Run verification and apply self-healing fixes immediately
    Run {
        /// Suppress output unless errors occur
        #[arg(short, long)]
        quiet: bool,
    },
    /// Install and register background LaunchAgent (monitors Steam updates via launchd)
    Install,
    /// Uninstall and unload background LaunchAgent
    Uninstall,
    /// Run live lightweight watcher daemon in the foreground
    Watch {
        /// Polling interval in seconds (default: 3)
        #[arg(short, long, default_value_t = 3)]
        interval: u64,
    },
}

pub fn run(action: GuardAction) -> Result<()> {
    match action {
        GuardAction::Status => {
            ui::header("Steam Update Guard Status");
            let status = guard::check_guard_status();
            ui::status_bool("Steam.app installed:", status.steam_installed);
            ui::kv(
                "Ad-hoc Codesignature:",
                if status.adhoc_signed {
                    "✓ Valid"
                } else {
                    "✗ Revoked / Missing (run 'nucleon guard run')"
                },
            );
            ui::kv(
                "Info.plist Injection:",
                if status.plist_patched {
                    "✓ Valid"
                } else {
                    "✗ Missing (run 'nucleon guard run')"
                },
            );
            ui::kv(
                "Hook Dylib Present:",
                if status.hook_dylib_present {
                    "✓ Present & Signed"
                } else {
                    "✗ Missing / Unsigned (run 'nucleon guard run')"
                },
            );
            ui::kv(
                "WebUI Chunk Patches:",
                if status.webui_patched {
                    "✓ Patched"
                } else {
                    "✗ Unpatched / Overwritten (run 'nucleon guard run')"
                },
            );
            ui::kv(
                "LaunchAgent Installed:",
                if status.launchagent_installed {
                    "✓ Yes"
                } else {
                    "○ No (run 'nucleon guard install')"
                },
            );
            ui::kv(
                "LaunchAgent Running:",
                if status.launchagent_loaded {
                    "✓ Loaded"
                } else {
                    "○ Not loaded"
                },
            );
            if let Some(b) = status.detected_steam_build {
                ui::kv(
                    "Signatures Cached:",
                    if status.signatures_cached {
                        format!("✓ Cached for build {}", b)
                    } else {
                        format!(
                            "○ Not cached for build {} (auto-generates on launch/guard run)",
                            b
                        )
                    },
                );
            }
            if let Some(ts) = status.last_heal_timestamp {
                ui::kv(
                    "Last Guard Verification:",
                    format!("{} (unix timestamp)", ts),
                );
            }
        }
        GuardAction::Run { quiet } => {
            if !quiet {
                ui::header("Running Steam Update Guard verification and heal...");
            }
            let hook_dylib = super::find_hook_dylib();
            let res = guard::run_guard_once(hook_dylib.as_deref())?;
            if !quiet {
                if res.no_action_needed {
                    ui::success("Steam signatures, hook injection, and WebUI patches are fully healthy. No action needed.");
                } else {
                    if res.hook_deployed {
                        ui::success("Deployed Nucleon hook dylib into Steam bundle");
                    }
                    if res.plist_fixed {
                        ui::success("Restored DYLD_INSERT_LIBRARIES in Steam Info.plist");
                    }
                    if res.resigned {
                        ui::success("Re-signed Steam.app and binaries with ad-hoc signature");
                    }
                    if res.webui_patched_count > 0 {
                        ui::success(format!(
                            "Re-applied patches to {} WebUI chunk(s) and cleared CEF cache",
                            res.webui_patched_count
                        ));
                    }
                    if res.mappings_cleaned {
                        ui::success("Cleaned native macOS games from CompatToolMapping");
                    }
                    if res.signatures_cached {
                        ui::success(
                            "Automatically scanned steamclient.dylib and cached signatures locally",
                        );
                    }
                }
            }
        }
        GuardAction::Install => {
            ui::header("Installing Nucleon Steam Guard LaunchAgent...");
            let hook_dylib = super::find_hook_dylib();
            if let Some(ref dylib) = hook_dylib {
                let _ = fs::copy(dylib, paths::bridge_dir().join("nucleon.dylib"));
            }
            let plist_path = guard::install_launchagent(None)?;
            ui::success(format!(
                "Created and loaded LaunchAgent: {}",
                plist_path.display()
            ));
            ui::success("Steam updates to /Applications/Steam.app or steamui will now automatically trigger self-healing.");
        }
        GuardAction::Uninstall => {
            ui::header("Uninstalling Nucleon Steam Guard LaunchAgent...");
            guard::uninstall_launchagent()?;
            ui::success("Unloaded and removed LaunchAgent plist");
        }
        GuardAction::Watch { interval } => {
            ui::header(format!(
                "Starting foreground Steam Guard watcher (polling every {}s)...",
                interval
            ));
            println!("Press Ctrl+C to stop.");
            let term = Arc::new(AtomicBool::new(false));
            let term_clone = Arc::clone(&term);
            ctrlc::set_handler(move || {
                term_clone.store(true, Ordering::SeqCst);
            })
            .ok();
            let hook_dylib = super::find_hook_dylib();
            guard::run_guard_watcher(term, Duration::from_secs(interval), hook_dylib.as_deref())?;
            println!("Watcher stopped.");
        }
    }

    Ok(())
}
