use super::application::SettingsArgs;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nucleon_core::{
    application::Application,
    launch_profile::{Authentication, LaunchProfile},
};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

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
            profile.settings.resolve_runtime()?;
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
    profile.settings.resolve_runtime()?;
    Ok(profile)
}

fn dispatch_command(
    options: &Options,
    auth: Vec<String>,
    environment: &BTreeMap<String, String>,
) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    nucleon_core::launch_profile::clean_profile_command(&mut command);
    command.args(["app", "dispatch", &options.name]);
    for (key, value) in [
        ("--exe", &options.executable),
        ("--prefix", &options.prefix),
        ("--cwd", &options.cwd),
    ] {
        if let Some(value) = value {
            command.arg(key).arg(value);
        }
    }
    // These are typed overrides only. Saved settings (including scaler input/renderer)
    // are loaded unchanged by dispatch instead of flattening them into backend-only flags.
    let settings = serde_json::to_value(options.settings.clone().settings()?)?;
    for (field, flag) in [
        ("wine", "--wine"),
        ("emulator", "--emulator"),
        ("engine", "--engine"),
        ("graphics_api", "--graphics-api"),
    ] {
        if let Some(value) = settings[field].as_str() {
            command.args([flag, value]);
        }
    }
    if let Some(value) = settings["hud"].as_bool() {
        command.arg(format!("--hud={value}"));
    }
    if let Some(value) = settings["upscaler"]["backend"].as_str() {
        command.args(["--upscaler", value]);
    }
    command
        .envs(environment)
        .arg("--")
        .args(auth)
        .args(&options.arguments);
    Ok(command)
}

fn launch(options: Options, dispatch: bool) -> Result<()> {
    let mut profile = selected(&options)?;
    if options.dry_run {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "name":options.name,"executable":profile.application.executable,"prefix":profile.prefix,
                "cwd":profile.application.cwd,"settings":profile.settings,"authentication":profile.authentication,
                "environment_keys":profile.environment.keys().collect::<Vec<_>>(),
                "arguments":nucleon_core::launch_profile::redact_arguments(&profile.application.arguments.iter().chain(&options.arguments).cloned().collect::<Vec<_>>()),
                "authenticated":false,"executed":false
            }))?
        );
        return Ok(());
    }
    if dispatch {
        profile.application.arguments.extend(options.arguments);
        return nucleon_runner::run_standalone(nucleon_runner::StandaloneRequest {
            application: profile.application,
            bottle: None,
            settings: profile.settings,
            prefix: profile.prefix,
        });
    }
    let auth = profile.authentication_arguments()?;
    let mut environment = auth.environment;
    environment.extend(profile.environment);
    let mut command = dispatch_command(&options, auth.arguments, &environment)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("Could not execute Nucleon's native application launcher")
    }
    #[cfg(not(unix))]
    {
        anyhow::ensure!(command.status()?.success(), "Application launch failed");
        Ok(())
    }
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
            arguments: vec!["--game-option".into(), "a b".into(), "".into()],
        };
        let command = dispatch_command(
            &options,
            vec!["-AUTH_PASSWORD=token value".into()],
            &BTreeMap::new(),
        )
        .unwrap();
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
                "--exe",
                "/other path/ü Game.exe",
                "--prefix",
                "/prefix with spaces",
                "--",
                "-AUTH_PASSWORD=token value",
                "--game-option",
                "a b",
                ""
            ]
        );
        assert_ne!(command.get_program(), "sh");
        assert_ne!(command.get_program(), "python3");
    }
}
