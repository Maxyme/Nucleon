//! Saved standalone launches. Store tools supply arguments, never the runner command.
use crate::application::{Application, Settings, validate_name, working_directory};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchProfile {
    pub schema: u32,
    pub application: Application,
    #[serde(default)]
    pub settings: Settings,
    pub prefix: Option<PathBuf>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub authentication: Option<Authentication>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase", deny_unknown_fields)]
pub enum Authentication {
    Legendary {
        binary: PathBuf,
        app_id: String,
        config: Option<PathBuf>,
    },
}

#[derive(Default)]
pub struct AuthenticatedLaunch {
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

impl LaunchProfile {
    pub fn path(name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(crate::paths::support_dir()
            .join("applications")
            .join(format!("{name}.json")))
    }

    pub fn load(name: &str) -> Result<Self> {
        let path = Self::path(name)?;
        crate::configuration::optional_json(&path)?.with_context(|| {
            format!("Application {name:?} not found; use 'nucleon app add' or 'nucleon app import'")
        })
    }

    /// Resolve relative paths against the imported document, not the current launch directory.
    pub fn normalize(&mut self, base: &Path) -> Result<()> {
        ensure!(self.schema == 1, "Unsupported application profile schema");
        let resolve = |p: &Path| {
            if p.is_absolute() {
                p.to_owned()
            } else {
                base.join(p)
            }
        };
        self.application = {
            let mut app = Application::new(
                &resolve(&self.application.executable),
                self.application.arguments.clone(),
                self.application.cwd.as_deref().map(resolve).as_deref(),
            )?;
            app.upscaler = self.application.upscaler.clone();
            app
        };
        self.prefix = self
            .prefix
            .as_deref()
            .map(|p| working_directory(&resolve(p)))
            .transpose()?;
        if let Some(wine) = &self.settings.wine {
            // Bare IDs stay portable; paths are anchored to the imported profile.
            if wine.contains('/') {
                self.settings.wine = Some(
                    resolve(Path::new(wine))
                        .canonicalize()?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        self.settings.validate()?;
        if let Some(scaler) = &mut self.settings.upscaler
            && let Some(renderer) = &scaler.renderer
        {
            scaler.renderer = Some(
                self.application
                    .executable
                    .parent()
                    .context("Executable has no directory")?
                    .join(renderer)
                    .canonicalize()
                    .context("Upscaler renderer not found")?,
            );
        }
        for (key, value) in &self.environment {
            ensure!(
                !key.is_empty() && !key.contains(['=', '\0']) && !value.contains('\0'),
                "Invalid launch environment variable"
            );
            ensure!(
                !matches!(
                    key.as_str(),
                    "WINEPREFIX"
                        | "WINEARCH"
                        | "NUCLEON_WINE"
                        | "NUCLEON_ENGINE"
                        | "NUCLEON_SUPPORT_DIR"
                ) && !key.starts_with("STEAM_")
                    && !key.starts_with("Steam"),
                "Use typed settings instead of environment variable {key}"
            );
        }
        if let Some(Authentication::Legendary {
            binary,
            app_id,
            config,
        }) = &mut self.authentication
        {
            ensure!(
                !app_id.trim().is_empty() && !app_id.starts_with('-') && !app_id.contains('\0'),
                "Invalid Legendary app ID"
            );
            *binary = resolve(binary)
                .canonicalize()
                .context("Legendary executable not found")?;
            ensure!(binary.is_file(), "Legendary executable must be a file");
            *config = config
                .as_deref()
                .map(|p| working_directory(&resolve(p)))
                .transpose()?;
        }
        Ok(())
    }

    pub fn save(&self, name: &str, replace: bool) -> Result<()> {
        let path = Self::path(name)?;
        fs::create_dir_all(path.parent().unwrap())?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .context("Another command is updating this application")?;
        ensure!(
            replace || !path.exists(),
            "Application {name:?} exists; use --replace"
        );
        crate::fs_util::atomic_write_file(&path, serde_json::to_vec_pretty(self)?)
    }

    /// JSON mode returns before Legendary launches Wine, wrappers or pre-launch commands.
    pub fn authentication_arguments(&self) -> Result<AuthenticatedLaunch> {
        let Some(Authentication::Legendary {
            binary,
            app_id,
            config,
        }) = &self.authentication
        else {
            return Ok(AuthenticatedLaunch::default());
        };
        let mut command = Command::new(binary);
        clean_profile_command(&mut command);
        command
            .args([
                "launch",
                app_id,
                "--skip-version-check",
                "--no-wine",
                "--json",
                "--override-exe",
            ])
            .arg(&self.application.executable);
        if let Some(config) = config {
            command.env("LEGENDARY_CONFIG_PATH", config);
        }
        command.stdin(std::process::Stdio::null());
        let output = command
            .output()
            .context("Could not request Legendary authentication")?;
        // Neither stderr nor JSON is included in errors: they may contain live credentials.
        ensure!(
            output.status.success(),
            "Legendary authentication failed ({}). Sign in through Heroic/Legendary and try again",
            output.status
        );
        parse_legendary(&output.stdout, &self.application.executable)
    }
}

/// Reset another application's launch controls before applying this saved profile.
pub fn clean_profile_command(command: &mut Command) {
    crate::launch::clean_command(command);
    crate::launch::clean_standalone_command(command);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if (name.starts_with("NUCLEON_") && name != "NUCLEON_SUPPORT_DIR")
            || ["FSR4_", "D3DM_", "D3DMETAL_"]
                .iter()
                .any(|p| name.starts_with(p))
        {
            command.env_remove(key);
        }
    }
}

fn parse_legendary(bytes: &[u8], expected: &Path) -> Result<AuthenticatedLaunch> {
    #[derive(Deserialize)]
    struct Parameters {
        game_directory: PathBuf,
        game_executable: PathBuf,
        game_parameters: Vec<String>,
        user_parameters: Vec<String>,
        egl_parameters: Vec<String>,
        #[serde(default)]
        environment: BTreeMap<String, String>,
    }
    let p: Parameters = serde_json::from_slice(bytes).map_err(|_| {
        anyhow::anyhow!("Legendary returned invalid launch JSON; credentials were not logged")
    })?;
    let expected = expected
        .canonicalize()
        .context("Configured executable is unavailable")?;
    ensure!(
        p.game_directory
            .join(&p.game_executable)
            .canonicalize()
            .ok()
            .as_deref()
            == Some(expected.as_path()),
        "Legendary resolved a different executable; check the saved installation and EXE path"
    );
    Ok(AuthenticatedLaunch {
        arguments: p
            .game_parameters
            .into_iter()
            .chain(p.user_parameters)
            .chain(p.egl_parameters)
            .collect(),
        environment: p.environment,
    })
}

/// Keep exchange codes out of diagnostic command traces as well as saved profiles.
pub fn redact_arguments(args: &[String]) -> Vec<String> {
    let mut hide_next = false;
    args.iter()
        .map(|arg| {
            if hide_next {
                hide_next = false;
                return "<redacted>".into();
            }
            let key = arg.split('=').next().unwrap_or(arg).to_ascii_lowercase();
            if [
                "auth_password",
                "auth_token",
                "access_token",
                "exchange_code",
                "password",
            ]
            .iter()
            .any(|secret| key.trim_start_matches('-') == *secret)
            {
                if let Some((key, _)) = arg.split_once('=') {
                    format!("{key}=<redacted>")
                } else {
                    hide_next = true;
                    arg.clone()
                }
            } else {
                arg.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legendary_arguments_preserve_boundaries_and_reject_a_different_exe() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Game with ünicode.exe");
        fs::write(&exe, b"fixture").unwrap();
        let json = serde_json::json!({
            "game_directory":dir.path(),"game_executable":exe.file_name().unwrap().to_str().unwrap(),
            "game_parameters":["a b", "$(not-a-shell)", ""],"user_parameters":["--fullscreen"],
            "egl_parameters":["-AUTH_PASSWORD=secret"],"launch_command":["untrusted-wrapper"],
            "pre_launch_command":"untrusted hook"
        });
        let args = parse_legendary(&serde_json::to_vec(&json).unwrap(), &exe)
            .unwrap()
            .arguments;
        assert_eq!(
            args,
            [
                "a b",
                "$(not-a-shell)",
                "",
                "--fullscreen",
                "-AUTH_PASSWORD=secret"
            ]
        );
        assert!(!args.iter().any(|s| s.contains("untrusted")));
        assert!(
            parse_legendary(
                &serde_json::to_vec(&json).unwrap(),
                &dir.path().join("other.exe")
            )
            .is_err()
        );
        let error = parse_legendary(b"invalid secret credentials", &exe)
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("secret"));
    }

    #[test]
    fn import_paths_are_anchored_and_reserved_environment_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Different Game.exe");
        fs::write(&exe, crate::runtime_checks::test_pe_image(0x8664)).unwrap();
        fs::create_dir(dir.path().join("prefix")).unwrap();
        let mut p: LaunchProfile = serde_json::from_value(serde_json::json!({
            "schema":1,"application":{"executable":"Different Game.exe","arguments":["a b"],"cwd":"."},
            "prefix":"prefix","settings":{"upscaler":{"backend":"fsr2","renderer":"Different Game.exe"}},"environment":{"CUSTOM":"value=with equals"},"authentication":null
        })).unwrap();
        p.normalize(dir.path()).unwrap();
        assert_eq!(p.application.executable, exe.canonicalize().unwrap());
        assert_eq!(
            p.settings.upscaler.as_ref().unwrap().renderer.as_ref(),
            Some(&exe.canonicalize().unwrap())
        );
        assert_eq!(
            p.prefix,
            Some(dir.path().join("prefix").canonicalize().unwrap())
        );
        p.environment.insert("WINEPREFIX".into(), "wrong".into());
        assert!(p.normalize(dir.path()).is_err());
        assert!(LaunchProfile::path("../escape").is_err());
    }

    #[test]
    fn diagnostic_redaction_hides_both_auth_argument_forms() {
        let args = [
            "-AUTH_PASSWORD=secret",
            "--access_token",
            "other secret",
            "visible",
            "",
        ]
        .map(String::from);
        assert_eq!(
            redact_arguments(&args),
            [
                "-AUTH_PASSWORD=<redacted>",
                "--access_token",
                "<redacted>",
                "visible",
                ""
            ]
        );
    }
}
