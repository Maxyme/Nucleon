use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand, ValueEnum};
use nucleon_core::application::{Application, Bottle, Settings};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Args)]
pub struct SettingsArgs {
    /// game, fsr2, fsr3.1, metalfx, fsr4-native-lite, fsr4-native or xess
    #[arg(long)]
    upscaler: Option<nucleon_core::upscaler::Backend>,
    /// Exact Wine ID or complete runtime path (overrides the bottle setting)
    #[arg(long)]
    wine: Option<String>,
    #[arg(long)]
    emulator: Option<nucleon_core::runtime_selection::Emulator>,
    /// auto, gptk (bundled Intel runtime), dxmt, dxvk, vkd3d, kosmickrisp or wine
    #[arg(short, long)]
    engine: Option<String>,
    /// auto, dx7, dx9, dx10, dx11, dx12, vulkan or opengl
    #[arg(long)]
    graphics_api: Option<String>,
    /// Enable Metal HUD, or use --hud=false to disable
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    hud: Option<bool>,
}

impl SettingsArgs {
    fn settings(self) -> Result<Settings> {
        let settings = Settings {
            upscaler: self.upscaler.map(nucleon_core::upscaler::Settings::selected),
            wine: self.wine, emulator: self.emulator, engine: self.engine,
            graphics_api: self.graphics_api.map(|value| serde_json::from_value(serde_json::Value::String(value))
                .context("Choose auto, dx7, dx9, dx10, dx11, dx12, vulkan or opengl for --graphics-api")).transpose()?,
            hud: self.hud,
        };
        settings.validate()?;
        Ok(settings)
    }
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Windows EXE; relative paths resolve from the invoking directory
    pub executable: PathBuf,
    /// Use an existing named environment (create with 'nucleon bottle create')
    #[arg(long)]
    pub bottle: Option<String>,
    /// Reuse an initialized Intel/Rosetta Wine prefix in place
    #[arg(long, conflicts_with = "bottle")]
    pub prefix: Option<PathBuf>,
    /// Working directory; defaults to the EXE's parent
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    #[command(flatten)]
    pub settings: SettingsArgs,
    /// Arguments passed unchanged to the Windows program, after --
    #[arg(last = true)]
    pub arguments: Vec<String>,
}

pub fn run(args: RunArgs) -> Result<()> {
    nucleon_runner::run_standalone(nucleon_runner::StandaloneRequest {
        application: Application::new(&args.executable, args.arguments, args.cwd.as_deref())?,
        bottle: args.bottle,
        settings: args.settings.settings()?,
        prefix: args.prefix,
    })
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SettingField {
    Upscaler,
    Wine,
    Emulator,
    Engine,
    GraphicsApi,
    Hud,
}

#[derive(Debug, Subcommand)]
pub enum BottleAction {
    /// Create a named environment, pinning its selected Wine; prefix is prepared on first run
    Create {
        name: String,
        #[command(flatten)]
        settings: SettingsArgs,
    },
    /// List environments and saved applications
    List,
    /// Show settings, saved applications and the environment's data directory
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Change defaults; --clear unsets selected fields, --reset clears all preferences
    Configure {
        name: String,
        #[command(flatten)]
        settings: SettingsArgs,
        #[arg(long)]
        reset: bool,
        #[arg(long, value_enum)]
        clear: Vec<SettingField>,
    },
    /// Save an application or installer entry; does not execute it
    Add {
        name: String,
        application: String,
        executable: PathBuf,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Replace this saved entry explicitly
        #[arg(long)]
        replace: bool,
        /// Save a scaler preference for this application only
        #[arg(long)]
        upscaler: Option<nucleon_core::upscaler::Backend>,
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Launch a saved entry, with optional one-launch setting and directory overrides
    Run {
        name: String,
        application: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[command(flatten)]
        settings: SettingsArgs,
        /// Append arguments to the saved arguments, after --
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Remove only a saved application entry; installed files and saves are retained
    Forget { name: String, application: String },
}

pub fn bottle(action: BottleAction) -> Result<()> {
    match action {
        BottleAction::Create { name, settings } => {
            let bottle = Bottle::create(&name, settings.settings()?)?;
            println!(
                "Created bottle {name}. Data: {}",
                bottle.data_directory()?.display()
            );
        }
        BottleAction::List => {
            let bottles = Bottle::list()?;
            if bottles.is_empty() {
                println!("No bottles. Create one with 'nucleon bottle create <name>'.");
            }
            for bottle in bottles {
                println!(
                    "{}  ({} saved applications)  {}",
                    bottle.name,
                    bottle.applications.len(),
                    bottle.data_directory()?.display()
                );
            }
        }
        BottleAction::Show { name, json } => {
            let bottle = Bottle::load(&name)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"bottle":bottle,"directory":Bottle::directory(&name)?,"data_directory":bottle.data_directory()?})
                    )?
                );
            } else {
                println!(
                    "Bottle: {name}\nData: {}\nSettings: {}",
                    bottle.data_directory()?.display(),
                    serde_json::to_string_pretty(&bottle.settings)?
                );
                for (name, app) in bottle.applications {
                    println!("  {name}: {}", app.executable.display());
                }
            }
        }
        BottleAction::Configure {
            name,
            settings,
            reset,
            clear,
        } => {
            let changes = settings.settings()?;
            let resolve = changes.wine.is_some() || changes.emulator.is_some();
            Bottle::update(&name, |bottle| {
                if reset {
                    bottle.settings = Settings::default();
                }
                for field in clear {
                    match field {
                        SettingField::Wine => bottle.settings.wine = None,
                        SettingField::Emulator => bottle.settings.emulator = None,
                        SettingField::Engine => bottle.settings.engine = None,
                        SettingField::GraphicsApi => bottle.settings.graphics_api = None,
                        SettingField::Hud => bottle.settings.hud = None,
                        SettingField::Upscaler => bottle.settings.upscaler = None,
                    }
                }
                bottle.settings = changes.with_defaults(bottle.settings.clone());
                if resolve {
                    let runtime = bottle.settings.resolve_runtime()?;
                    bottle.settings.wine = Some(runtime.root.to_string_lossy().into_owned());
                }
                Ok(())
            })?;
            println!("Updated bottle {name}.");
        }
        BottleAction::Add {
            name,
            application,
            executable,
            cwd,
            replace,
            upscaler,
            arguments,
        } => {
            nucleon_core::application::validate_name(&application)?;
            let mut entry = Application::new(&executable, arguments, cwd.as_deref())?;
            entry.upscaler = upscaler.map(nucleon_core::upscaler::Settings::selected);
            Bottle::update(&name, |bottle| {
                ensure!(
                    replace || !bottle.applications.contains_key(&application),
                    "Application {application:?} already exists; use --replace to update it"
                );
                bottle.applications.insert(application.clone(), entry);
                Ok(())
            })?;
            println!("Saved {application} in bottle {name}.");
        }
        BottleAction::Run {
            name,
            application,
            cwd,
            settings,
            arguments,
        } => {
            let bottle = Bottle::load(&name)?;
            let mut entry = bottle.applications.get(&application).with_context(|| format!("No application {application:?} in bottle {name:?}; use 'nucleon bottle show {name}'"))?.clone();
            if let Some(cwd) = cwd {
                entry.cwd = Some(cwd);
            }
            entry.arguments.extend(arguments);
            nucleon_runner::run_standalone(nucleon_runner::StandaloneRequest {
                application: entry,
                bottle: Some(name),
                settings: settings.settings()?,
                prefix: None,
            })?;
        }
        BottleAction::Forget { name, application } => {
            Bottle::update(&name, |bottle| {
                ensure!(
                    bottle.applications.remove(&application).is_some(),
                    "Application {application:?} was not found"
                );
                Ok(())
            })?;
            println!("Forgot {application}. Bottle data and saves are retained.");
        }
    }
    Ok(())
}
