//! Explicit native DLSS-G passthrough recipes loaded from `assets/frame-generation.toml`.
//! This is separate from OptiScaler's FSR/XeSS FG.
use crate::{fs_util::atomic_write_file, launch::LaunchPlan, repo_assets::ASSETS, upscaler};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::OnceLock,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct IniRule {
    file: String,
    section: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    metalfx: BTreeMap<String, String>,
    #[serde(default)]
    off: BTreeMap<String, String>,
}

impl IniRule {
    fn changes(&self, setting: Setting) -> &BTreeMap<String, String> {
        match setting {
            Setting::Metalfx => &self.metalfx,
            Setting::Off => &self.off,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Defaults {
    #[serde(default)]
    files: Vec<IniRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeSpec {
    executables: Vec<String>,
    #[serde(default)]
    experimental: bool,
    #[serde(default)]
    files: Vec<IniRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema: u32,
    #[serde(default)]
    strip_suffixes: Vec<String>,
    #[serde(default)]
    defaults: Defaults,
    recipes: BTreeMap<String, RecipeSpec>,
}

fn parse_catalog(text: &str) -> Result<Catalog> {
    let catalog: Catalog =
        toml::from_str(text).context("Invalid assets/frame-generation.toml")?;
    ensure!(
        catalog.schema == 1 && !catalog.recipes.is_empty(),
        "Unsupported frame-generation catalog schema"
    );
    for (id, recipe) in &catalog.recipes {
        ensure!(
            !recipe.executables.is_empty() && !recipe.files.is_empty(),
            "Frame-generation recipe {id} must define executables and INI rules"
        );
        for rule in catalog.defaults.files.iter().chain(&recipe.files) {
            ensure!(
                !rule.file.is_empty()
                    && !rule.file.contains(['/', '\\'])
                    && !rule.section.trim().is_empty(),
                "Invalid INI rule in frame-generation recipe {id}"
            );
        }
    }
    Ok(catalog)
}

fn catalog() -> Result<&'static Catalog> {
    static CATALOG: OnceLock<std::result::Result<Catalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            parse_catalog(
                ASSETS
                    .get_file("frame-generation.toml")
                    .expect("Embedded frame-generation catalog")
                    .contents_utf8()
                    .expect("UTF-8 frame-generation catalog"),
            )
            .map_err(|error| format!("{error:#}"))
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error}"))
}

fn find_recipe(renderer: &Path) -> Result<(String, &'static RecipeSpec)> {
    let name = renderer.file_name().unwrap_or_default().to_string_lossy();
    let catalog = catalog()?;
    for (id, spec) in &catalog.recipes {
        if spec
            .executables
            .iter()
            .any(|candidate| name.eq_ignore_ascii_case(candidate))
        {
            return Ok((id.clone(), spec));
        }
    }
    anyhow::bail!(
        "No frame-generation settings recipe for {}",
        renderer.display()
    )
}

#[derive(Debug, Serialize)]
pub struct Resolved {
    pub setting: Setting,
    pub configuration_directories: Vec<PathBuf>,
    recipe: String,
    pub experimental: bool,
}

/// Resolve before authentication or prefix mutation; unsupported games are not guessed.
pub fn resolve(
    setting: Setting,
    plan: &LaunchPlan,
    scaler: &upscaler::Resolved,
) -> Result<Resolved> {
    let (recipe, spec) = find_recipe(&scaler.renderer)?;
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
        recipe,
        experimental: spec.experimental,
        configuration_directories: configuration_directories(&plan.prefix, &scaler.renderer)?,
    })
}

fn candidate_folder_names(renderer: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for ancestor in renderer.ancestors() {
        if ancestor
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("Binaries"))
            && let Some(parent) = ancestor.parent().and_then(|p| p.file_name())
        {
            names.push(parent.to_string_lossy().to_string());
        }
    }
    if let Some(stem) = renderer.file_stem().and_then(|s| s.to_str()) {
        names.push(stem.to_string());
        for suffix in &catalog()?.strip_suffixes {
            if let Some(stripped) = stem.strip_suffix(suffix.as_str()) {
                names.push(stripped.to_string());
            }
        }
    }
    let mut deduped = Vec::new();
    for name in names {
        if !deduped
            .iter()
            .any(|e: &String| e.eq_ignore_ascii_case(&name))
        {
            deduped.push(name);
        }
    }
    Ok(deduped)
}

fn configuration_directories(prefix: &Path, renderer: &Path) -> Result<Vec<PathBuf>> {
    let prefix = prefix
        .canonicalize()
        .context("Start the game once before configuring frame generation")?;
    let users = prefix.join("drive_c/users");
    let mut directories = Vec::new();
    let candidates = candidate_folder_names(renderer)?;

    if users.is_dir() {
        for user in fs::read_dir(&users).context("Start the game once to create its settings")? {
            let user = user?;
            let appdata_local = user.path().join("AppData/Local");
            if !appdata_local.is_dir() {
                continue;
            }
            for candidate in &candidates {
                let candidate_config = appdata_local.join(candidate).join("Saved/Config");
                if let Ok(entries) = fs::read_dir(&candidate_config) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.to_ascii_lowercase().starts_with("windows")
                            && entry.path().join("GameUserSettings.ini").is_file()
                        {
                            let canon = entry.path().canonicalize()?;
                            ensure!(
                                canon.starts_with(&prefix),
                                "Game settings escape the selected prefix"
                            );
                            directories.push(canon);
                        }
                    }
                }
            }
        }
    }

    directories.sort();
    directories.dedup();
    ensure!(
        !directories.is_empty(),
        "Start the game once in this prefix before configuring frame generation"
    );
    Ok(directories)
}

impl Resolved {
    /// Called with the prefix lock held. Back up originals once; preserve all unrelated settings.
    pub fn apply(&self) -> Result<()> {
        let catalog = catalog()?;
        let spec = catalog
            .recipes
            .get(&self.recipe)
            .with_context(|| format!("Unknown frame-generation recipe {}", self.recipe))?;

        // Group rules by filename in declaration order while merging sections that appear in both defaults and recipe.
        let mut file_order: Vec<(String, bool)> = Vec::new();
        let mut file_sections: BTreeMap<String, Vec<(String, BTreeMap<String, String>)>> =
            BTreeMap::new();

        for rule in catalog.defaults.files.iter().chain(&spec.files) {
            let changes = rule.changes(self.setting);
            if changes.is_empty() {
                continue;
            }
            if let Some((_, req)) = file_order
                .iter_mut()
                .find(|(f, _)| f.eq_ignore_ascii_case(&rule.file))
            {
                *req |= rule.required;
            } else {
                file_order.push((rule.file.clone(), rule.required));
            }
            let sections = file_sections.entry(rule.file.clone()).or_default();
            if let Some((_, existing)) = sections
                .iter_mut()
                .find(|(s, _)| s.eq_ignore_ascii_case(&rule.section))
            {
                existing.extend(changes.iter().map(|(k, v)| (k.clone(), v.clone())));
            } else {
                sections.push((rule.section.clone(), changes.clone()));
            }
        }

        let mut edits = Vec::new();
        for directory in &self.configuration_directories {
            for (file_name, required) in &file_order {
                let path = directory.join(file_name);
                ensure!(
                    !path.is_symlink(),
                    "Game settings cannot be symlinks: {}",
                    path.display()
                );
                let original = match fs::read_to_string(&path) {
                    Ok(text) => Some(text),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => None,
                    Err(e) => return Err(e).with_context(|| format!("Read {}", path.display())),
                };
                let mut updated = original.clone().unwrap_or_default();
                if let Some(sections) = file_sections.get(file_name) {
                    for (section, changes) in sections {
                        updated = update_ini(&updated, section, changes);
                    }
                }
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
                    Err(e) => return Err(e).context("Back up game frame-generation settings"),
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
                    format!("Configure game frame generation; rollback errors: {rollback_errors:?}")
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
            recipe: "Talos2".into(),
            experimental: false,
            configuration_directories: configuration_directories(
                temp.path(),
                &temp.path().join("Talos2.exe"),
            )
            .unwrap(),
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
    fn habitat_recipe_selects_dlss_without_changing_unrelated_settings() {
        let original = "[Other]\nFrameGeneration=7\n[/Script/Habitat.HabitatLocalSettings]\nESSTechnologyType=AMDFSR3\nbAMDFSR3Enabled=True\nbAMDFSR3FrameGenEnabled=True\nNvidiaDLSSFrameGenerationMode=0\n";
        let (_, spec) = find_recipe(Path::new("StillWakesTheDeep.exe")).unwrap();
        let rule = &spec.files[0];
        let enabled = update_ini(original, &rule.section, rule.changes(Setting::Metalfx));
        assert!(enabled.contains("[Other]\nFrameGeneration=7"));
        assert!(enabled.contains("NvidiaDLSSFrameGenerationMode=1"));
        assert!(enabled.contains("ESSTechnologyType=NVidiaDLSS"));
        assert!(enabled.contains("bAMDFSR3FrameGenEnabled=False"));
        let changes = rule.changes(Setting::Off);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes["NvidiaDLSSFrameGenerationMode"], "0");
        assert!(find_recipe(Path::new("TheInvincible-Win64-Shipping.exe")).is_ok());
        assert!(find_recipe(Path::new("Unknown-Win64-Shipping.exe")).is_err());
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
        let dummy_renderer = temp.path().join("Talos2.exe");
        assert!(configuration_directories(temp.path(), &dummy_renderer).is_err());
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("GameUserSettings.ini"), "").unwrap();
        let user = temp
            .path()
            .join("drive_c/users/wine/AppData/Local/Talos2/Saved/Config");
        fs::create_dir_all(&user).unwrap();
        std::os::unix::fs::symlink(external.path(), user.join("Windows")).unwrap();
        assert!(configuration_directories(temp.path(), &dummy_renderer).is_err());
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
