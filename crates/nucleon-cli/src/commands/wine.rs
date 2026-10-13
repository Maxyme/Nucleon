use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::{steam, wine};
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum WineAction {
    /// List all discovered Wine runtimes and show the currently active desired version
    List,
    /// Select desired active Wine version for Steam (e.g. 'crossover', 'staging', 'dxmt', or path)
    Use {
        /// Wine identifier, name keyword, or custom path
        version: String,
    },
    /// Inspect details of active Wine runtime and Steam registration
    Status,
    /// Register a custom Wine runtime with a custom name
    Add {
        /// Unique identifier or name for the runtime (e.g. 'crossover-24', 'proton-ge')
        name: String,
        /// Path to Wine installation directory (containing bin/wine or Contents/Resources/wine)
        path: PathBuf,
        /// Immediately set this runtime as the active Wine runtime
        #[arg(short, long)]
        use_now: bool,
    },
    /// Unregister a custom Wine runtime
    #[command(alias = "rm", alias = "unregister")]
    Remove {
        /// Identifier or name of the custom Wine runtime to unregister
        name: String,
    },
    /// Point Nucleon to a custom Wine installation directory
    SetPath {
        /// Path to Wine installation directory (containing bin/wine or Contents/Resources/wine)
        path: PathBuf,
    },
    /// Reset active Wine to default recommended primary runtime
    Reset,
    /// Clear configured custom Wine path
    ClearPath,
    /// Manage Wine graphical translation backends (KosmicKrisp, VKD3D-Proton, D7VK)
    Backends {
        #[command(subcommand)]
        action: super::backends::BackendsAction,
    },
}

pub fn run(action: WineAction) -> Result<()> {
    match action {
        WineAction::List | WineAction::Status => {
            ui::header("Discovered Wine Runtimes (Heroic, Homebrew, Whisky, CrossOver, Custom)");
            let runtimes = wine::discover_wine_runtimes();
            let active = wine::get_active_wine_runtime();

            if runtimes.is_empty() {
                ui::info("No Wine runtimes detected.");
                println!(
                    "  To configure a custom Wine runtime: nucleon wine set-path /path/to/wine"
                );
            } else {
                let custom_ids: std::collections::HashSet<String> = wine::load_custom_wines()
                    .into_iter()
                    .map(|r| r.id)
                    .collect();

                println!("  Found {} runtime(s):\n", runtimes.len());
                for rt in &runtimes {
                    let is_active = active.as_ref().map(|a| a.root == rt.root).unwrap_or(false);
                    let is_custom = rt.id == "custom" || custom_ids.contains(&rt.id);
                    let ver = rt.version.as_deref().unwrap_or("Unknown version");
                    let custom_marker = if is_custom { " [CUSTOM]" } else { "" };
                    let active_marker = if is_active { " [ACTIVE]" } else { "" };
                    let symbol = if is_active { "●" } else { "○" };
                    println!(
                        "  {} {:<16} - {} [{}] {}{}\n    Location: {}",
                        symbol,
                        rt.id,
                        rt.name,
                        ver,
                        custom_marker,
                        active_marker,
                        rt.root.display()
                    );
                }
                let active_name = active.as_ref().map(|a| a.name.as_str()).unwrap_or("None");
                println!(
                    r#"
  Steam UI Tool: 'Nucleon (Wine + WineD3D OpenGL)' -> currently using '{active_name}'
  To switch active Wine:        nucleon wine use <staging|crossover|path>
  To add a custom Wine:         nucleon wine add <name> <path> [--use-now]
  To remove a custom Wine:      nucleon wine remove <name>
  Per-game Steam Launch Option: NUCLEON_WINE=crossover %command%
                                NUCLEON_WINE=/Applications/CrossOver.app %command%"#
                );
            }
        }
        WineAction::Add {
            name,
            path,
            use_now,
        } => {
            ui::header(format!(
                "Registering custom Wine runtime '{}' from {}...",
                name,
                path.display()
            ));
            let rt = wine::add_custom_wine(&name, &path)?;
            ui::success(format!(
                "Validated and registered Wine runtime: {} [{}]",
                rt.name,
                rt.version.as_deref().unwrap_or("Unknown")
            ));
            ui::success(format!("Identifier: {}", rt.id));
            ui::success(format!("Root path:  {}", rt.root.display()));

            if use_now {
                let _ = wine::set_active_wine(&rt.id);
                ui::success("Set as active Wine runtime for Steam.");
            } else {
                println!("\nTo switch to this Wine: nucleon wine use {}", rt.id);
            }
        }
        WineAction::Remove { name } => {
            ui::header(format!("Unregistering custom Wine runtime '{}'...", name));
            if wine::remove_custom_wine(&name)? {
                ui::success(format!("Successfully unregistered '{}'.", name));
            } else {
                ui::warn(format!("No custom Wine runtime found matching '{}'.", name));
            }
        }
        WineAction::Use { version } => {
            ui::header(format!("Setting active Wine runtime to '{}'...", version));
            let rt = wine::set_active_wine(&version)?;
            ui::success(format!(
                "Active Wine set to: {} [{}]",
                rt.name,
                rt.version.as_deref().unwrap_or("Unknown")
            ));
            ui::success(format!("Root path: {}", rt.root.display()));
            if steam::is_wine_tool_registered() {
                ui::success("Steam compatibility tool 'Nucleon (Wine)' updated immediately.");
            } else {
                ui::info("Run 'nucleon setup' to register 'Nucleon (Wine)' in Steam.");
            }
        }
        WineAction::Reset => {
            wine::clear_active_wine()?;
            ui::success("Reset active Wine to default primary runtime.");
            if let Some(primary) = wine::find_primary_wine_runtime() {
                ui::success(format!(
                    "Now using: {} [{}]",
                    primary.name,
                    primary.version.as_deref().unwrap_or("Unknown")
                ));
            }
        }
        WineAction::SetPath { path } => {
            ui::header(format!("Registering custom Wine path: {}", path.display()));
            let rt = wine::set_custom_wine_path(&path)?;
            ui::success(format!(
                "Registered custom Wine: {} [{}]",
                rt.name,
                rt.version.as_deref().unwrap_or("Unknown")
            ));
            ui::success(format!("Wine root: {}", rt.root.display()));
            println!("To activate this Wine: nucleon wine use custom");
        }
        WineAction::ClearPath => {
            wine::clear_custom_wine_path()?;
            ui::success("Cleared custom Wine path configuration.");
        }
        WineAction::Backends { action } => super::backends::run(action)?,
    }

    Ok(())
}
