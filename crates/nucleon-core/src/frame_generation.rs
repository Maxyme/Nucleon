//! Explicit native DLSS-G passthrough recipes. This is separate from OptiScaler's FSR/XeSS FG.
use crate::{fs_util::atomic_write_file, launch::LaunchPlan, upscaler};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Setting {
    Off,
    Metalfx,
}
impl FromStr for Setting {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        serde_json::from_value(serde_json::Value::String(value.into()))
            .context("Choose metalfx or off for --frame-generation")
    }
}

#[derive(Debug, Serialize)]
pub struct Resolved {
    pub setting: Setting,
    pub configuration_directories: Vec<PathBuf>,
}

/// Resolve before authentication or prefix mutation; unsupported games are not guessed.
pub fn resolve(
    setting: Setting,
    plan: &LaunchPlan,
    scaler: &upscaler::Resolved,
) -> Result<Resolved> {
    ensure!(
        scaler
            .renderer
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Talos2.exe")
                || name.eq_ignore_ascii_case("Talos2-Win64-Shipping.exe")),
        "Frame-generation settings are currently validated only for Talos 2"
    );
    if setting == Setting::Metalfx {
        ensure!(
            scaler.settings.backend == upscaler::Backend::Metalfx
                && scaler.api == "DirectX12"
                && scaler.bridge.is_some()
                && plan.runtime.mode == crate::runtime::ExecutionMode::Intel,
            "MetalFX frame generation requires --upscaler metalfx, DX12 and the matching Intel GPTK runtime"
        );
    }
    Ok(Resolved {
        setting,
        configuration_directories: configuration_directories(&plan.prefix)?,
    })
}

fn configuration_directories(prefix: &Path) -> Result<Vec<PathBuf>> {
    let prefix = prefix
        .canonicalize()
        .context("Start Talos 2 once before configuring frame generation")?;
    let users = prefix.join("drive_c/users");
    let mut directories = Vec::new();
    // Wine's active username differs between runtimes. Configure only existing Talos
    // settings, including older users that may become active after a runtime switch.
    for user in fs::read_dir(&users).context("Start Talos 2 once to create its settings")? {
        let directory = user?
            .path()
            .join("AppData/Local/Talos2/Saved/Config/Windows");
        if directory.join("GameUserSettings.ini").is_file() {
            let directory = directory.canonicalize()?;
            ensure!(
                directory.starts_with(&prefix),
                "Talos settings escape the selected prefix"
            );
            directories.push(directory);
        }
    }
    directories.sort();
    directories.dedup();
    ensure!(
        !directories.is_empty(),
        "Start Talos 2 once in this prefix before configuring frame generation"
    );
    Ok(directories)
}

impl Resolved {
    /// Called with the prefix lock held. Back up originals once; preserve all unrelated settings.
    pub fn apply(&self) -> Result<()> {
        let value = if self.setting == Setting::Metalfx {
            "1"
        } else {
            "0"
        };
        let mut edits = Vec::new();
        for directory in &self.configuration_directories {
            for (name, section, keys) in [
                (
                    "Engine.ini",
                    "ConsoleVariables",
                    vec!["r.Streamline.InitializePlugin", "r.Streamline.DLSSG.Enable"],
                ),
                (
                    "GameUserSettings.ini",
                    "/Script/Talos2.TalosGameUserSettings",
                    vec!["FrameGeneration"],
                ),
            ] {
                let path = directory.join(name);
                ensure!(
                    !path.is_symlink(),
                    "Talos settings cannot be symlinks: {}",
                    path.display()
                );
                let original = match fs::read_to_string(&path) {
                    Ok(text) => Some(text),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound && name == "Engine.ini" => {
                        None
                    }
                    Err(e) => return Err(e).with_context(|| format!("Read {}", path.display())),
                };
                let changes = keys
                    .into_iter()
                    .map(|key| (key.to_owned(), value.to_owned()))
                    .collect();
                let updated = update_ini(original.as_deref().unwrap_or(""), section, &changes);
                if original.as_deref() != Some(updated.as_str()) {
                    edits.push((path, original, updated));
                }
            }
        }
        // Complete backups before making any of the requested edits.
        for (path, original, _) in &edits {
            if let Some(original) = original {
                let backup = path.with_extension("ini.nucleon-framegen.bak");
                match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&backup)
                {
                    Ok(mut file) => {
                        file.write_all(original.as_bytes())?;
                        file.sync_all()?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e).context("Back up Talos frame-generation settings"),
                }
            }
        }
        for (index, (path, _, updated)) in edits.iter().enumerate() {
            if let Err(error) = atomic_write_file(path, updated.as_bytes()) {
                let mut rollback_errors = Vec::new();
                for (path, original, _) in &edits[..index] {
                    let result = match original {
                        Some(text) => atomic_write_file(path, text.as_bytes()),
                        None => fs::remove_file(path).map_err(Into::into),
                    };
                    if let Err(error) = result {
                        rollback_errors.push(error.to_string());
                    }
                }
                return Err(error).with_context(|| {
                    format!(
                        "Configure Talos frame generation; rollback errors: {rollback_errors:?}"
                    )
                });
            }
        }
        Ok(())
    }
}

fn update_ini(text: &str, target: &str, changes: &BTreeMap<String, String>) -> String {
    let mut output = String::new();
    let mut section = String::new();
    let mut remaining = changes.clone();
    let mut found = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if section.eq_ignore_ascii_case(target) {
                for (key, value) in &remaining {
                    output.push_str(&format!("{key}={value}\n"));
                }
                remaining.clear();
            }
            section = trimmed[1..trimmed.len() - 1].to_owned();
            found |= section.eq_ignore_ascii_case(target);
        }
        if section.eq_ignore_ascii_case(target)
            && !trimmed.starts_with([';', '#'])
            && let Some((key, _)) = trimmed.split_once('=')
            && let Some((canonical, value)) = changes
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key.trim()))
        {
            output.push_str(&format!("{}={value}\n", key.trim()));
            remaining.remove(canonical);
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if !found {
        output.push_str(&format!("\n[{target}]\n"));
    }
    for (key, value) in remaining {
        output.push_str(&format!("{key}={value}\n"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enable_disable_preserves_other_sections_and_original_backups() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp
            .path()
            .join("drive_c/users/wine/AppData/Local/Talos2/Saved/Config/Windows");
        fs::create_dir_all(&directory).unwrap();
        let original = "[Other]\nFrameGeneration=7\n[/Script/Talos2.TalosGameUserSettings]\nFrameGeneration=0\nFOV=90\n";
        fs::write(directory.join("GameUserSettings.ini"), original).unwrap();
        let resolved = Resolved {
            setting: Setting::Metalfx,
            configuration_directories: configuration_directories(temp.path()).unwrap(),
        };
        assert!(!directory.join("Engine.ini").exists()); // resolution is read only
        resolved.apply().unwrap();
        let enabled = fs::read_to_string(directory.join("GameUserSettings.ini")).unwrap();
        assert!(enabled.contains("[Other]\nFrameGeneration=7"));
        assert!(enabled.contains("FrameGeneration=1\nFOV=90"));
        assert!(
            fs::read_to_string(directory.join("Engine.ini"))
                .unwrap()
                .contains("r.Streamline.InitializePlugin=1")
        );
        Resolved {
            setting: Setting::Off,
            ..resolved
        }
        .apply()
        .unwrap();
        assert!(
            fs::read_to_string(directory.join("GameUserSettings.ini"))
                .unwrap()
                .contains("FrameGeneration=0\nFOV=90")
        );
        assert_eq!(
            fs::read_to_string(directory.join("GameUserSettings.ini.nucleon-framegen.bak"))
                .unwrap(),
            original
        );
    }
    #[test]
    fn unsupported_titles_and_unvalidated_upscalers_fail_before_config_writes() {
        use crate::{
            detector::{GraphicsApi, TargetEngine},
            runtime::{self, ExecutionMode},
        };
        let (temp, runtime) = runtime::fixture(ExecutionMode::Intel);
        let prefix = temp.path().join("prefix");
        let directory = prefix.join("drive_c/users/wine/AppData/Local/Talos2/Saved/Config/Windows");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("GameUserSettings.ini"), "").unwrap();
        let plan = LaunchPlan::new(
            runtime,
            prefix,
            TargetEngine::WineStaging,
            Default::default(),
            true,
            Some(false),
            &GraphicsApi::DirectX12,
        )
        .unwrap();
        let mut scaler = upscaler::Resolved {
            settings: upscaler::Settings::selected(upscaler::Backend::Metalfx),
            executable: temp.path().join("Talos2.exe"),
            renderer: temp.path().join("Talos2-Win64-Shipping.exe"),
            input: upscaler::Input::Dlss,
            api: "DirectX12".into(),
            runtime: plan.runtime.root.clone(),
            bridge: Some(temp.path().join("nvngx-on-metalfx.dll")),
            provider: None,
            native_provider: None,
            patcher: true,
            experimental: true,
        };
        assert!(resolve(Setting::Metalfx, &plan, &scaler).is_ok());
        assert!(!directory.join("Engine.ini").exists());
        scaler.settings.backend = upscaler::Backend::Fsr412;
        assert!(resolve(Setting::Metalfx, &plan, &scaler).is_err());
        assert!(resolve(Setting::Off, &plan, &scaler).is_ok());
        scaler.renderer = temp.path().join("Other.exe");
        assert!(resolve(Setting::Off, &plan, &scaler).is_err());
    }

    #[test]
    fn settings_reject_unknown_choices_and_paths_outside_the_prefix() {
        assert!("fsr".parse::<Setting>().is_err());
        assert_eq!("off".parse::<Setting>().unwrap(), Setting::Off);
        let temp = tempfile::tempdir().unwrap();
        assert!(configuration_directories(temp.path()).is_err());
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("GameUserSettings.ini"), "").unwrap();
        let user = temp
            .path()
            .join("drive_c/users/wine/AppData/Local/Talos2/Saved/Config");
        fs::create_dir_all(&user).unwrap();
        std::os::unix::fs::symlink(external.path(), user.join("Windows")).unwrap();
        assert!(configuration_directories(temp.path()).is_err());
    }
    #[test]
    fn missing_and_duplicate_keys_are_set_only_in_the_target_section() {
        let values = BTreeMap::from([("Enabled".into(), "0".into()), ("Other".into(), "1".into())]);
        let text = "[Target]\n; Enabled=9\nEnabled=1\nEnabled=2\n[Unrelated]\nEnabled=3\n";
        let updated = update_ini(text, "Target", &values);
        assert!(
            updated.contains("; Enabled=9\nEnabled=0\nEnabled=0\nOther=1\n[Unrelated]\nEnabled=3")
        );
        assert_eq!(update_ini(&updated, "Target", &values), updated);
    }
}
