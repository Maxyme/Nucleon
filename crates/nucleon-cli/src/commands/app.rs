use super::application::SettingsArgs;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nucleon_core::{
    application::Application,
    launch_profile::{Authentication, LaunchProfile},
};
use std::{path::PathBuf, process::Command};

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Save a reusable EXE launch; does not run Wine or authenticate
    Add {
        name: String,
        executable: PathBuf,
        #[arg(long)]
        prefix: Option<PathBuf>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[command(flatten)]
        settings: SettingsArgs,
        /// Per-application environment, as NAME=value (repeatable)
        #[arg(long = "env", value_parser = parse_env)]
        environment: Vec<(String, String)>,
        /// Optional Epic authentication through the installed Legendary executable
        #[arg(long, requires = "legendary")]
        legendary_app: Option<String>,
        #[arg(long, requires = "legendary_app")]
        legendary: Option<PathBuf>,
        #[arg(long, requires = "legendary_app")]
        legendary_config: Option<PathBuf>,
        #[arg(long)]
        replace: bool,
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Import a generic JSON launch profile; relative paths use the JSON file's directory
    Import {
        name: String,
        file: PathBuf,
        #[arg(long)]
        replace: bool,
    },
    /// List saved application names and EXEs
    List,
    /// Display saved settings; never obtains live store credentials
    Show { name: String },
    /// Validate a profile and obtain fresh store arguments, without running Wine or the game
    Check { name: String },
    /// Launch a saved EXE, optionally overriding its path, runtime or arguments
    Run(Options),
    #[command(hide = true)]
    Dispatch(Options),
}

#[derive(Debug, Clone, Args)]
pub struct Options {
    name: String,
    /// Use a different executable for this launch; retain the saved environment
    #[arg(long = "exe")]
    executable: Option<PathBuf>,
    #[arg(long)]
    prefix: Option<PathBuf>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[command(flatten)]
    settings: SettingsArgs,
    /// Validate and preview without authentication, Wine, or game execution
    #[arg(long)]
    dry_run: bool,
    /// Stop this launch after the given number of seconds (diagnostic only)
    #[arg(long)]
    run_seconds: Option<std::num::NonZeroU64>,
    #[arg(last = true)]
    arguments: Vec<String>,
}

fn parse_env(value: &str) -> std::result::Result<(String, String), String> {
    value
        .split_once('=')
        .map(|(k, v)| (k.into(), v.into()))
        .filter(|(k, _): &(String, String)| !k.is_empty())
        .ok_or_else(|| "Expected NAME=value".into())
}

pub fn run(action: Action) -> Result<()> {
    match action {
        Action::Add {
            name,
            executable,
            prefix,
            cwd,
            settings,
            environment,
            legendary_app,
            legendary,
            legendary_config,
            replace,
            arguments,
        } => {
            let mut profile = LaunchProfile {
                schema: 1,
                application: Application::new(&executable, arguments, cwd.as_deref())?,
                settings: settings.settings()?,
                prefix,
                environment: environment.into_iter().collect(),
                authentication: legendary_app.zip(legendary).map(|(app_id, binary)| {
                    Authentication::Legendary {
                        app_id,
                        binary,
                        config: legendary_config,
                    }
                }),
            };
            profile.normalize(&std::env::current_dir()?)?;
            profile.save(&name, replace)?;
            println!("Saved {name}. Launch with 'nucleon app run {name}'.");
        }
        Action::Import {
            name,
            file,
            replace,
        } => {
            let file = file.canonicalize()?;
            let mut profile: LaunchProfile = serde_json::from_slice(&std::fs::read(&file)?)
                .context("Invalid application profile")?;
            profile.normalize(file.parent().context("Profile has no parent directory")?)?;
            profile.save(&name, replace)?;
            println!("Imported {name}. No game was started.");
        }
        Action::List => {
            let dir = nucleon_core::paths::support_dir().join("applications");
            if !dir.exists() {
                return Ok(());
            }
            let mut entries = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                if entry.path().extension().is_some_and(|e| e == "json") {
                    let name = entry
                        .path()
                        .file_stem()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    let profile = LaunchProfile::load(&name)?;
                    println!("{name}: {}", profile.application.executable.display());
                }
            }
        }
        Action::Show { name } => println!(
            "{}",
            serde_json::to_string_pretty(&LaunchProfile::load(&name)?)?
        ),
        Action::Check { name } => {
            let mut profile = LaunchProfile::load(&name)?;
            profile.normalize(&std::env::current_dir()?)?;
            resolve(&profile)?;
            let arguments = profile.authentication_arguments()?;
            println!(
                "{name}: profile valid; {} store arguments obtained. Credentials were not printed. No game was started.",
                arguments.arguments.len()
            );
        }
        Action::Run(options) => return launch(options, false),
        Action::Dispatch(options) => return launch(options, true),
    }
    Ok(())
}

fn selected(options: &Options) -> Result<LaunchProfile> {
    let mut profile = LaunchProfile::load(&options.name)?;
    if let Some(exe) = &options.executable {
        profile.application.executable = exe.clone();
    }
    if let Some(prefix) = &options.prefix {
        profile.prefix = Some(prefix.clone());
    }
    if let Some(cwd) = &options.cwd {
        profile.application.cwd = Some(cwd.clone());
    }
    profile.settings = options
        .settings
        .clone()
        .settings()?
        .with_defaults(profile.settings);
    profile.normalize(&std::env::current_dir()?)?;
    Ok(profile)
}

const LAUNCH_REQUEST: &str = "NUCLEON_LAUNCH_REQUEST";

fn resolve(profile: &LaunchProfile) -> Result<nucleon_runner::ResolvedStandalone> {
    nucleon_runner::resolve_standalone(nucleon_runner::StandaloneRequest {
        application: profile.application.clone(),
        bottle: None,
        settings: profile.settings.clone(),
        prefix: profile.prefix.clone(),
    })
}

fn dispatch_command(options: &Options, profile: &LaunchProfile) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    nucleon_core::launch_profile::clean_profile_command(&mut command);
    command.args(["app", "dispatch", &options.name]);
    if options.dry_run {
        command.arg("--dry-run");
    }
    if let Some(seconds) = options.run_seconds {
        command.args(["--run-seconds", &seconds.to_string()]);
    }
    // Freeze the selected profile before re-exec; no second profile load, default
    // selection, or command-line flattening can change it. Never put it in argv.
    command
        .envs(&profile.environment)
        .env(LAUNCH_REQUEST, serde_json::to_string(profile)?)
        .arg("--")
        .args(&options.arguments);
    if let Some(seconds) = options.run_seconds {
        command.env("NUCLEON_RUN_SECONDS", seconds.to_string());
    }
    Ok(command)
}

fn launch(options: Options, dispatch: bool) -> Result<()> {
    if !dispatch {
        let profile = selected(&options)?;
        let mut command = dispatch_command(&options, &profile)?;
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            return Err(command.exec())
                .context("Could not execute Nucleon's native application launcher");
        }
        #[cfg(not(unix))]
        {
            anyhow::ensure!(command.status()?.success(), "Application launch failed");
            return Ok(());
        }
    }
    // Preview and execution both resolve inside the same clean environment with
    // the saved variables applied. Preview never obtains store credentials.
    let profile = match std::env::var(LAUNCH_REQUEST) {
        Ok(json) => serde_json::from_str::<LaunchProfile>(&json)
            .context("Invalid application dispatch request")?,
        Err(std::env::VarError::NotPresent) => selected(&options)?,
        Err(error) => return Err(error.into()),
    };
    let mut resolved = resolve(&profile)?;
    if options.dry_run {
        resolved.append_arguments(options.arguments);
        let mut preview = resolved.preview();
        preview["name"] = options.name.into();
        preview["settings"] = serde_json::to_value(&profile.settings)?;
        preview["authentication"] = serde_json::to_value(&profile.authentication)?;
        preview["environment_keys"] =
            serde_json::to_value(profile.environment.keys().collect::<Vec<_>>())?;
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }
    let auth = profile.authentication_arguments()?;
    resolved.append_arguments(auth.arguments);
    resolved.append_arguments(options.arguments);
    let mut environment = auth.environment;
    environment.extend(profile.environment);
    if let Some(seconds) = options.run_seconds {
        environment.insert("NUCLEON_RUN_SECONDS".into(), seconds.to_string());
    }
    resolved.run(environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_preserves_paths_and_live_arguments_without_a_shell() {
        let options = Options {
            name: "any-game".into(),
            executable: Some("/other path/ü Game.exe".into()),
            prefix: Some("/prefix with spaces".into()),
            cwd: None,
            settings: SettingsArgs::default(),
            dry_run: false,
            run_seconds: std::num::NonZeroU64::new(45),
            arguments: vec!["--game-option".into(), "a b".into(), "".into()],
        };
        let profile = LaunchProfile {
            schema: 1,
            application: Application {
                frame_generation: None,
                executable: options.executable.clone().unwrap(),
                arguments: vec!["saved argument".into()],
                cwd: None,
                upscaler: None,
            },
            settings: Default::default(),
            prefix: options.prefix.clone(),
            environment: [
                ("GAME_OPTION".into(), "a b".into()),
                ("NUCLEON_RUN_SECONDS".into(), "999".into()),
            ]
            .into(),
            authentication: None,
        };
        let command = dispatch_command(&options, &profile).unwrap();
        let args = command
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "app",
                "dispatch",
                "any-game",
                "--run-seconds",
                "45",
                "--",
                "--game-option",
                "a b",
                ""
            ]
        );
        let snapshot = command
            .get_envs()
            .find(|(k, _)| *k == LAUNCH_REQUEST)
            .unwrap()
            .1
            .unwrap();
        let frozen: LaunchProfile = serde_json::from_str(snapshot.to_str().unwrap()).unwrap();
        assert_eq!(
            frozen.application.executable,
            profile.application.executable
        );
        assert_eq!(frozen.prefix, profile.prefix);
        assert_eq!(frozen.application.arguments, profile.application.arguments);
        assert_eq!(frozen.environment, profile.environment);
        assert_eq!(
            command
                .get_envs()
                .find(|(k, _)| *k == "NUCLEON_RUN_SECONDS")
                .unwrap()
                .1
                .unwrap(),
            "45"
        );
        assert_ne!(command.get_program(), "sh");
        assert_ne!(command.get_program(), "python3");
    }
}
