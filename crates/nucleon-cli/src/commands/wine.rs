use super::ui;
use anyhow::{Context, Result, ensure};
use clap::{Subcommand, ValueEnum};
use nucleon_core::{steam, wine, wine_build};
use std::num::NonZeroUsize;
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum WineAction {
    /// Install a published Wine/FEX version, HTTPS archive, or local runtime
    Install {
        /// wine-fex, an HTTPS archive URL, a local archive, or a runtime directory
        source: String,
        /// Exact Wine/FEX release version (required when source is wine-fex)
        #[arg(long)]
        version: Option<String>,
        /// Optional archive checksum; required for URLs without release metadata
        #[arg(long)]
        sha256: Option<String>,
        /// Select the installed runtime for the experimental FEX Steam tool
        #[arg(long)]
        register: bool,
    },
    /// Validate a runtime descriptor and print resolved component paths
    Inspect {
        runtime: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Manage the opt-in native ARM64/FEX Steam tool
    Fex {
        #[command(subcommand)]
        action: FexAction,
    },
    /// Download and register the recommended Wine build without selecting it
    Fetch {
        #[arg(long, default_value = "11.18")]
        version: String,
        /// New package directory; defaults to Nucleon's managed wines directory
        #[arg(long)]
        output: Option<PathBuf>,
        /// Use a cached, checksum-verified binary archive instead of downloading
        #[arg(long)]
        archive: Option<PathBuf>,
        /// Unique registration name; defaults to wine-devel-<version>
        #[arg(long)]
        name: Option<String>,
    },
    /// Rebuild a patched module and register a separate copy; requires native build tools
    Patch {
        #[arg(value_enum)]
        kind: PatchKind,
        /// Registered Wine identifier or path to the reviewed Wine runtime
        runtime: String,
        /// New runtime directory; must not exist
        #[arg(long)]
        output: Option<PathBuf>,
        /// Optional checksum-verified Wine source archive
        #[arg(long)]
        source_archive: Option<PathBuf>,
        #[arg(long, default_value = "8")]
        jobs: NonZeroUsize,
        /// Unique registration name for the copy
        #[arg(long)]
        name: Option<String>,
    },
    /// List all discovered Wine runtimes and show the currently active desired version
    List,
    /// Select the Intel default, or register a FEX default without changing Intel Wine
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

#[derive(Subcommand, Debug, Clone)]
pub enum FexAction {
    /// Install a complete local runtime outside the build tree, retaining earlier generations
    Install {
        runtime: String,
        /// Required for .tar.gz, .tar.zst or .zip archives
        #[arg(long)]
        sha256: Option<String>,
        /// Point only the experimental Steam option to this installed generation
        #[arg(long)]
        register: bool,
    },
    /// Hash every installed file and check the native dependency closure
    Verify { runtime: Option<PathBuf> },
    /// Register a complete native runtime without changing global Wine or game mappings
    Register { runtime: String },
    /// Inspect the experimental tool's pinned runtime
    Status,
    /// Remove only the experimental Steam tool
    Unregister,
    /// Update a game's ARM64/FEX profile (leaves the ordinary profile intact)
    Profile {
        appid: std::num::NonZeroU32,
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        graphics_api: Option<String>,
        #[arg(long)]
        steam_protocol: Option<String>,
        #[arg(long)]
        steam_helper: Option<bool>,
        #[arg(long)]
        overlay: Option<bool>,
    },
}

fn install_runtime(
    source: &str,
    version: Option<&str>,
    sha256: Option<&str>,
    register: bool,
) -> Result<()> {
    ui::info("Verifying and installing Wine/FEX runtime components...");
    let installed = nucleon_core::runtime_releases::install(source, version, sha256)?;
    ui::success(format!("Installed runtime: {}", installed.display()));
    if register {
        steam::fex::register(&installed, &std::env::current_exe()?)?;
        ui::success("Experimental FEX Steam option now uses this runtime.");
    }
    Ok(())
}

fn run_fex(action: FexAction) -> Result<()> {
    use nucleon_core::{paths, profile::GameProfile};
    match action {
        FexAction::Install {
            runtime,
            sha256,
            register,
        } => {
            install_runtime(&runtime, None, sha256.as_deref(), register)?;
        }
        FexAction::Verify { runtime } => {
            let root = match runtime {
                Some(path) => path,
                None => {
                    steam::fex::load_selection(
                        &paths::steam_fex_compat_tools_dir().join(steam::fex::SELECTION_FILE),
                    )?
                    .root
                }
            };
            nucleon_core::managed_runtime::verify(&root)?;
            ui::success(format!(
                "Runtime integrity and native dependencies verified: {}",
                root.display()
            ));
        }
        FexAction::Register { runtime } => {
            let root = if std::path::Path::new(&runtime).is_dir() {
                PathBuf::from(&runtime)
            } else {
                wine::resolve_wine_runtime_by_query(&runtime)
                    .context("Runtime not found; supply a complete ARM64/FEX runtime path")?
                    .root
            };
            steam::fex::register(&root, &std::env::current_exe()?)?;
            ui::success(format!("Registered '{}'.", steam::fex::DISPLAY_NAME));
            ui::info(
                "Select it in a game's Steam Compatibility settings. Restart Steam when convenient if it is not listed.",
            );
        }
        FexAction::Status => {
            let path = paths::steam_fex_compat_tools_dir().join(steam::fex::SELECTION_FILE);
            if path.exists() {
                let runtime = steam::fex::load_selection(&path)?;
                ui::kv(steam::fex::DISPLAY_NAME, runtime.root.display());
                ui::kv(
                    "Installation",
                    if runtime
                        .root
                        .join(nucleon_core::managed_runtime::RECEIPT)
                        .is_file()
                    {
                        "Managed; retained generation"
                    } else {
                        "External runtime"
                    },
                );
                ui::kv(
                    "Prefix family",
                    format!("{} (ABI {})", runtime.prefix.family, runtime.prefix.abi),
                );
            } else {
                ui::info("Experimental ARM64/FEX tool is not registered.");
            }
        }
        FexAction::Unregister => {
            steam::fex::unregister()?;
            ui::success("Removed experimental ARM64/FEX tool.");
        }
        FexAction::Profile {
            appid,
            engine,
            graphics_api,
            steam_protocol,
            steam_helper,
            overlay,
        } => {
            let dir = paths::game_profiles_dir(true);
            let file = dir.join(format!("{appid}.json"));
            let mut profile = GameProfile::load(&file)?
                .unwrap_or_default()
                .with_defaults(GameProfile::fex_default(&appid.to_string()));
            if let Some(value) = engine {
                nucleon_core::configuration::engine("--engine", &value)?;
                profile.engine = Some(value);
            }
            if let Some(value) = graphics_api {
                profile.graphics_api=Some(serde_json::from_value(serde_json::Value::String(value)).context("Choose auto, dx7, dx9, dx10, dx11, dx12, vulkan or opengl for --graphics-api")?);
            }
            if let Some(value) = steam_protocol {
                profile.steam_protocol = Some(
                    serde_json::from_value(serde_json::Value::String(value))
                        .context("Choose bridge or valve-redirect for --steam-protocol")?,
                );
            }
            if steam_helper.is_some() {
                profile.use_steam_shim = steam_helper;
            }
            if overlay.is_some() {
                profile.overlay = overlay;
            }
            std::fs::create_dir_all(&dir)?;
            nucleon_core::fs_util::atomic_write_file(&file, serde_json::to_vec_pretty(&profile)?)?;
            ui::success(format!("ARM64/FEX profile: {}", file.display()));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PatchKind {
    Rosetta,
    SteamRedirect,
}

impl From<PatchKind> for wine_build::WinePatch {
    fn from(kind: PatchKind) -> Self {
        match kind {
            PatchKind::Rosetta => Self::Rosetta,
            PatchKind::SteamRedirect => Self::SteamRedirect,
        }
    }
}

fn ensure_new_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty(),
        "Runtime registration name cannot be empty"
    );
    let id = name.trim().to_lowercase().replace([' ', '_'], "-");
    let registry = wine::custom_wines_file();
    let records = if registry.exists() {
        serde_json::from_str::<Vec<wine::CustomWineRecord>>(&std::fs::read_to_string(&registry)?)
            .context("Cannot register Wine while the custom-runtime registry is invalid")?
    } else {
        Vec::new()
    };
    ensure!(
        !records.iter().any(|runtime| runtime.id == id),
        "Runtime name '{name}' is already registered; use --name with a new identifier"
    );
    Ok(())
}

pub fn run(action: WineAction) -> Result<()> {
    match action {
        WineAction::Install {
            source,
            version,
            sha256,
            register,
        } => {
            install_runtime(&source, version.as_deref(), sha256.as_deref(), register)?;
        }
        WineAction::Inspect { runtime, json } => {
            let runtime = nucleon_core::runtime::ResolvedRuntime::inspect(&runtime)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({"schema": 1, "runtime": runtime}))?
                );
            } else {
                ui::kv("Runtime", runtime.root.display());
                ui::kv("Execution", format!("{:?}", runtime.mode));
            }
        }
        WineAction::Fex { action } => run_fex(action)?,
        WineAction::Fetch {
            version,
            output,
            archive,
            name,
        } => {
            let name = name.unwrap_or_else(|| format!("wine-devel-{version}"));
            ensure_new_name(&name)?;
            let output = output.unwrap_or_else(|| wine_build::default_runtime_path(&version, None));
            ui::header(format!("Fetching reviewed Wine {version}..."));
            let root = wine_build::fetch(&version, &output, archive.as_deref())?;
            ensure_new_name(&name)?;
            let runtime = wine::add_custom_wine(&name, &root)?;
            ui::success(format!(
                "Downloaded and registered {} at {}",
                runtime.id,
                runtime.root.display()
            ));
            ui::info("Active Wine selection was not changed.");
            println!(
                "Create a patched copy: nucleon wine patch rosetta {}",
                runtime.id
            );
        }
        WineAction::Patch {
            kind,
            runtime,
            output,
            source_archive,
            jobs,
            name,
        } => {
            let patch = wine_build::WinePatch::from(kind);
            let version = wine_build::RECOMMENDED_VERSION;
            let name = name.unwrap_or_else(|| format!("wine-devel-{version}-{}", patch.name()));
            ensure_new_name(&name)?;
            let base = wine::resolve_wine_runtime_by_query(&runtime).context(
                "Wine runtime not found; run nucleon wine fetch or supply an existing runtime path",
            )?;
            let output =
                output.unwrap_or_else(|| wine_build::default_runtime_path(version, Some(patch)));
            ui::header(format!("Building {} into a new Wine copy...", patch.name()));
            let root = wine_build::build(&wine_build::BuildOptions {
                runtime: base.root,
                output,
                patch,
                source_archive,
                jobs: jobs.get(),
            })?;
            ensure_new_name(&name)?;
            let runtime = wine::add_custom_wine(&name, &root)?;
            ui::success(format!("Registered {} at {}", runtime.id, root.display()));
            ui::info("Original runtime and active Wine selection were not changed.");
            match patch {
                wine_build::WinePatch::Rosetta => {
                    println!("Select when ready: nucleon wine use {}", runtime.id)
                }
                wine_build::WinePatch::SteamRedirect => {
                    ui::warn(
                        "Experimental: use only in a game-specific profile with NUCLEON_STEAMCLIENT_REDIRECT=1 and matching Valve/lsteamclient libraries.",
                    );
                    println!(
                        "Per-game launch option: NUCLEON_WINE={} NUCLEON_STEAMCLIENT_REDIRECT=1 %command%",
                        runtime.id
                    );
                }
            }
        }
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
                    match rt.capabilities() {
                        Ok(capabilities) => println!(
                            "    Execution: {:?}; guests: {:?}; Steam: {:?}",
                            capabilities.mode, capabilities.guests, capabilities.steam_protocol
                        ),
                        Err(error) => println!("    Runtime configuration error: {error:#}"),
                    }
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
                return run(WineAction::Use { version: rt.id });
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
            let requested = if version == "fex-preview" {
                nucleon_core::runtime_selection::named(&version)?
            } else {
                wine::resolve_wine_runtime_by_query(&version)
                    .context("Wine runtime was not found; run 'nucleon wine list'")?
                    .capabilities()?
            };
            if requested.mode == nucleon_core::runtime::ExecutionMode::Arm64Fex {
                steam::fex::register(&requested.root, &std::env::current_exe()?)?;
                ui::success("FEX default updated. The Intel/Rosetta default is unchanged.");
                return Ok(());
            }
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
            let capabilities = nucleon_core::runtime_selection::named(&path.to_string_lossy())?;
            if capabilities.mode == nucleon_core::runtime::ExecutionMode::Arm64Fex {
                steam::fex::register(&capabilities.root, &std::env::current_exe()?)?;
                ui::success("FEX default updated. The Intel/Rosetta default is unchanged.");
                return Ok(());
            }
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
