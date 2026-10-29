//! Standalone application identities and named Wine environments. No Steam mutations.
use crate::{
    configuration, fs_util::atomic_write_file, paths, profile::GameProfile,
    runtime_selection::Emulator,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub upscaler: Option<crate::upscaler::Settings>,
    pub frame_generation: Option<crate::frame_generation::Setting>,
    pub wine: Option<String>,
    pub emulator: Option<Emulator>,
    pub engine: Option<String>,
    pub graphics_api: Option<crate::profile::GraphicsPreference>,
    pub hud: Option<bool>,
}

impl Settings {
    /// Explicit fields win; false and auto are intentional choices.
    pub fn with_defaults(self, defaults: Self) -> Self {
        Self {
            upscaler: self.upscaler.or(defaults.upscaler),
            frame_generation: self.frame_generation.or(defaults.frame_generation),
            wine: self.wine.or(defaults.wine),
            emulator: self.emulator.or(defaults.emulator),
            engine: self.engine.or(defaults.engine),
            graphics_api: self.graphics_api.or(defaults.graphics_api),
            hud: self.hud.or(defaults.hud),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(engine) = &self.engine {
            configuration::engine("engine", engine)?;
        }
        if let Some(wine) = &self.wine {
            ensure!(!wine.trim().is_empty(), "Wine selection cannot be empty");
        }
        Ok(())
    }

    pub fn profile(&self) -> GameProfile {
        GameProfile {
            upscaler: self.upscaler.clone(),
            frame_generation: self.frame_generation,
            wine: self.wine.clone(),
            emulator: self.emulator,
            engine: self.engine.clone(),
            graphics_api: self.graphics_api,
            hud: self.hud,
            overlay: Some(false),
            use_steam_shim: Some(false),
            ..Default::default()
        }
    }

    pub fn resolve_runtime(&self) -> Result<crate::runtime::ResolvedRuntime> {
        self.validate()?;
        match crate::runtime_selection::resolve(self.wine.as_deref(), self.emulator)? {
            Some(runtime) => Ok(runtime),
            None => crate::runtime_selection::standalone_default(),
        }
    }
}

/// Resolve ordinary EXE launches and upscaler previews with the same runtime,
/// target-API and component policy. This does not create any launch state.
pub fn resolve_launch(
    executable: &Path,
    data: &Path,
    prefix: Option<&Path>,
    settings: &Settings,
) -> Result<crate::launch::LaunchPlan> {
    use crate::{
        detector::TargetEngine,
        launch::{GraphicsComponents, LaunchPlan},
        runtime::GuestAbi,
    };
    settings.validate()?;
    let guest = crate::detector::executable_guest(executable)?;
    let runtime = settings.resolve_runtime()?;
    runtime.require_guest(guest == GuestAbi::X64)?;
    crate::runtime_selection::check_host(&runtime)?;
    let target = settings
        .upscaler
        .as_ref()
        .and_then(|s| s.renderer.as_ref())
        .map(|renderer| executable.parent().unwrap_or(Path::new(".")).join(renderer))
        .unwrap_or_else(|| executable.into());
    let api = settings
        .profile()
        .graphics_api(&target, &crate::detector::detect_target_engine(&target).api)?;
    let requested = settings
        .engine
        .as_deref()
        .map(|e| configuration::engine("--engine", e))
        .transpose()?
        .unwrap_or(TargetEngine::Auto);
    let mut components = if matches!(requested, TargetEngine::WineStaging | TargetEngine::Gptk) {
        GraphicsComponents::default()
    } else {
        GraphicsComponents::discover(&runtime, None, None)?
    };
    components.gptk = GraphicsComponents::bundled_gptk(&runtime);
    let engine = if requested == TargetEngine::Auto {
        components
            .available(&runtime, &api, guest == GuestAbi::X64)
            .select(&api)?
    } else {
        requested
    };
    ensure!(
        engine != TargetEngine::Gptk || (components.gptk && guest == GuestAbi::X64),
        "GPTK requires a complete compatible Intel Wine runtime with bundled D3DMetal and an x64 target; select it with --wine, or choose another engine"
    );
    let prefix = prefix
        .map(Path::to_path_buf)
        .unwrap_or_else(|| paths::game_prefix(data, &runtime));
    Ok(LaunchPlan::new(
        runtime,
        prefix,
        engine,
        components,
        guest == GuestAbi::X64,
        Some(false),
        &api,
    )?
    .without_steam())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Application {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upscaler: Option<crate::upscaler::Settings>,
    pub frame_generation: Option<crate::frame_generation::Setting>,
    pub executable: PathBuf,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub cwd: Option<PathBuf>,
}

impl Application {
    pub fn new(executable: &Path, arguments: Vec<String>, cwd: Option<&Path>) -> Result<Self> {
        let executable = executable
            .canonicalize()
            .with_context(|| format!("Resolve executable {}", executable.display()))?;
        crate::detector::executable_guest(&executable)?;
        Ok(Self {
            upscaler: None,
            frame_generation: None,
            executable,
            arguments,
            cwd: cwd.map(working_directory).transpose()?,
        })
    }
}

pub fn working_directory(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Resolve working directory {}", path.display()))?;
    ensure!(
        path.is_dir(),
        "Working directory is not a directory: {}",
        path.display()
    );
    Ok(path)
}

pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_')),
        "Names must contain 1–64 lowercase letters, digits, '-' or '_', starting with a letter or digit"
    );
    Ok(())
}

pub fn standalone_data(executable: &Path) -> Result<(String, PathBuf)> {
    // The canonical target is stable across launch directories and source symlinks.
    let canonical = executable.canonicalize()?;
    let identity = crate::staging::key(&canonical)?;
    Ok((
        format!("app-{identity}"),
        paths::support_dir()
            .join("standalone")
            .join(identity)
            .join("data"),
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bottle {
    pub schema: u32,
    pub name: String,
    pub settings: Settings,
    pub applications: BTreeMap<String, Application>,
}

impl Bottle {
    pub fn directory(name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        let path = paths::support_dir().join("bottles").join(name);
        ensure!(
            !path.is_symlink(),
            "Bottle directory cannot be a symlink: {}",
            path.display()
        );
        Ok(path)
    }

    pub fn data_directory(&self) -> Result<PathBuf> {
        Ok(Self::directory(&self.name)?.join("data"))
    }

    pub fn create(name: &str, mut settings: Settings) -> Result<Self> {
        let directory = Self::directory(name)?;
        // Pin the selected generation once, so later global changes do not move this bottle.
        let runtime = settings.resolve_runtime()?;
        settings.wine = Some(runtime.root.to_string_lossy().into_owned());
        let bottle = Self {
            schema: 1,
            name: name.into(),
            settings,
            applications: BTreeMap::new(),
        };
        fs::create_dir_all(directory.parent().context("Bottle has no parent")?)?;
        fs::create_dir(&directory).with_context(|| {
            format!("Cannot create bottle {name:?}; choose a new name if it already exists")
        })?;
        if let Err(error) = bottle.save() {
            // Remove only an empty directory reserved by this attempt. A published
            // configuration or concurrently created data is never removed.
            let _ = fs::remove_dir(&directory);
            return Err(error);
        }
        Ok(bottle)
    }

    pub fn load(name: &str) -> Result<Self> {
        let path = Self::directory(name)?.join("bottle.json");
        let bottle: Self = configuration::optional_json(&path)?.with_context(|| {
            format!("Bottle {name:?} was not found; create it with 'nucleon bottle create {name}'")
        })?;
        ensure!(
            bottle.schema == 1 && bottle.name == name,
            "Invalid bottle identity in {}",
            path.display()
        );
        bottle.settings.validate()?;
        for name in bottle.applications.keys() {
            validate_name(name)?;
        }
        Ok(bottle)
    }

    pub fn list() -> Result<Vec<Self>> {
        let entries = match fs::read_dir(paths::support_dir().join("bottles")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut bottles = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.path().is_dir() {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("Invalid bottle directory name"))?;
                bottles.push(Self::load(&name)?);
            }
        }
        bottles.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(bottles)
    }

    /// Serialize read/modify/write updates without locking or touching the Wine prefix.
    pub fn update(name: &str, operation: impl FnOnce(&mut Self) -> Result<()>) -> Result<Self> {
        let directory = Self::directory(name)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(".configuration.lock"))
            .with_context(|| format!("Open bottle {name:?}"))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .context("Another command is updating this bottle; try again")?;
        let mut bottle = Self::load(name)?;
        operation(&mut bottle)?;
        ensure!(
            bottle.name == name && bottle.schema == 1,
            "Bottle updates cannot change its identity"
        );
        bottle.settings.validate()?;
        bottle.save()?;
        Ok(bottle)
    }

    fn save(&self) -> Result<()> {
        atomic_write_file(
            Self::directory(&self.name)?.join("bottle.json"),
            serde_json::to_vec_pretty(self)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{self, ExecutionMode};

    use proptest::prelude::*;

    fn choices() -> impl Strategy<Value = Settings> {
        (
            prop::option::of(any::<bool>()),
            prop::option::of(any::<bool>()),
            prop::option::of("[a-z]{1,16}"),
            prop::option::of(0_usize..3),
            prop::option::of(0_usize..3),
            prop::option::of(0_usize..2),
            prop::option::of(0_usize..3),
        )
            .prop_map(|(hud, fg, wine, engine, api, emulator, scaler)| Settings {
                frame_generation: fg.map(|on| {
                    if on {
                        crate::frame_generation::Setting::Metalfx
                    } else {
                        crate::frame_generation::Setting::Off
                    }
                }),
                hud,
                wine,
                engine: engine.map(|i| ["auto", "wine", "gptk"][i].into()),
                graphics_api: api.map(|i| {
                    [
                        crate::profile::GraphicsPreference::Auto,
                        crate::profile::GraphicsPreference::Dx11,
                        crate::profile::GraphicsPreference::Dx12,
                    ][i]
                }),
                emulator: emulator.map(|i| [Emulator::Rosetta, Emulator::Fex][i]),
                upscaler: scaler.map(|i| {
                    crate::upscaler::Settings::selected(
                        [
                            crate::upscaler::Backend::Game,
                            crate::upscaler::Backend::Fsr31,
                            crate::upscaler::Backend::Metalfx,
                        ][i],
                    )
                }),
            })
    }

    proptest! {
        #[test]
        fn explicit_choices_survive_inherited_defaults(explicit in choices(), defaults in choices()) {
            let chosen = serde_json::to_value(&explicit).unwrap();
            let inherited = serde_json::to_value(&defaults).unwrap();
            let effective = serde_json::to_value(explicit.with_defaults(defaults)).unwrap();
            for (key, value) in chosen.as_object().unwrap() {
                let expected = if value.is_null() { &inherited[key] } else { value };
                prop_assert_eq!(&effective[key], expected);
            }
        }
        #[test]
        fn launch_app_and_bottle_precedence_is_associative(launch in choices(), app in choices(), bottle in choices()) {
            let sequential = launch.clone().with_defaults(app.clone()).with_defaults(bottle.clone());
            let layered = launch.with_defaults(app.with_defaults(bottle));
            prop_assert_eq!(serde_json::to_value(sequential).unwrap(), serde_json::to_value(layered).unwrap());
        }
    }

    #[test]
    fn bottles_pin_runtime_update_atomically_and_preserve_data() {
        if crate::test_support::run_in_isolated_support(
            "application::tests::bottles_pin_runtime_update_atomically_and_preserve_data",
        ) {
            return;
        }
        let (_runtime_dir, runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        let settings = Settings {
            wine: Some(runtime.root.display().to_string()),
            engine: Some("wine".into()),
            ..Default::default()
        };
        let bottle = Bottle::create("test-game", settings.clone()).unwrap();
        assert_eq!(bottle.settings.wine.as_deref(), runtime.root.to_str());
        assert!(Bottle::create("test-game", settings).is_err());
        fs::create_dir_all(bottle.data_directory().unwrap()).unwrap();
        fs::write(bottle.data_directory().unwrap().join("save"), b"keep").unwrap();
        Bottle::update("test-game", |b| {
            b.settings.hud = Some(false);
            Ok(())
        })
        .unwrap();
        let before = fs::read(Bottle::directory("test-game").unwrap().join("bottle.json")).unwrap();
        assert!(
            Bottle::update("test-game", |b| {
                b.settings.engine = Some("typo".into());
                Ok(())
            })
            .is_err()
        );
        assert_eq!(
            fs::read(Bottle::directory("test-game").unwrap().join("bottle.json")).unwrap(),
            before
        );
        assert_eq!(
            fs::read(bottle.data_directory().unwrap().join("save")).unwrap(),
            b"keep"
        );
        assert_eq!(Bottle::list().unwrap().len(), 1);
        assert_eq!(Bottle::load("test-game").unwrap().settings.hud, Some(false));
        let executable = runtime.root.join("saved-game.exe");
        let mut image = crate::runtime_checks::test_pe_image(0x8664);
        image[150..152].copy_from_slice(&2u16.to_le_bytes());
        fs::write(&executable, image).unwrap();
        let entry = Application::new(
            &executable,
            vec!["argument with spaces".into()],
            Some(&runtime.root),
        )
        .unwrap();
        Bottle::update("test-game", |b| {
            b.applications.insert("game".into(), entry);
            Ok(())
        })
        .unwrap();
        let saved = Bottle::load("test-game").unwrap();
        assert_eq!(
            saved.applications["game"].arguments,
            ["argument with spaces"]
        );
        assert_eq!(
            saved.applications["game"].cwd.as_deref(),
            Some(runtime.root.as_path())
        );
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(
                Bottle::directory("test-game")
                    .unwrap()
                    .join(".configuration.lock"),
            )
            .unwrap();
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
        assert!(Bottle::update("test-game", |_| Ok(())).is_err());
        drop(lock);
        Bottle::update("test-game", |b| {
            b.applications.remove("game");
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::read(bottle.data_directory().unwrap().join("save")).unwrap(),
            b"keep"
        );
        for name in ["../escape", "", ".", "Upper", "/tmp/a", "a/b"] {
            assert!(Bottle::directory(name).is_err());
        }
    }

    #[test]
    fn targets_validate_before_prefix_creation_and_identity_follows_canonical_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Game with ünicode.exe");
        for (machine, guest) in [
            (0x14c, runtime::GuestAbi::X86),
            (0x8664, runtime::GuestAbi::X64),
        ] {
            let mut image = crate::runtime_checks::test_pe_image(machine);
            image[150..152].copy_from_slice(&2u16.to_le_bytes());
            fs::write(&path, image).unwrap();
            assert_eq!(crate::detector::executable_guest(&path).unwrap(), guest);
            Application::new(
                &path,
                vec!["--option".into(), "value with spaces".into()],
                Some(temp.path()),
            )
            .unwrap();
        }
        let link = temp.path().join("alias.exe");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(
            standalone_data(&path).unwrap(),
            standalone_data(&link).unwrap()
        );
        let other = temp.path().join("Other.exe");
        fs::copy(&path, &other).unwrap();
        assert_ne!(
            standalone_data(&path).unwrap(),
            standalone_data(&other).unwrap()
        );
        fs::write(&path, b"not a Windows program").unwrap();
        assert!(Application::new(&path, vec![], None).is_err());
        let mut arm = crate::runtime_checks::test_pe_image(0xaa64);
        arm[150..152].copy_from_slice(&2u16.to_le_bytes());
        fs::write(&path, arm).unwrap();
        assert!(
            crate::detector::executable_guest(&path)
                .unwrap_err()
                .to_string()
                .contains("Unsupported executable architecture")
        );
        assert!(!temp.path().join("data").exists());
        let mut hybrid = crate::runtime_checks::test_pe_image(0x8664);
        hybrid.resize(768, 0);
        hybrid[150..152].copy_from_slice(&2u16.to_le_bytes());
        hybrid[134..136].copy_from_slice(&1u16.to_le_bytes());
        hybrid[260..264].copy_from_slice(&16u32.to_le_bytes());
        hybrid[344..348].copy_from_slice(&0x1000u32.to_le_bytes());
        hybrid[348..352].copy_from_slice(&208u32.to_le_bytes());
        hybrid[400..404].copy_from_slice(&256u32.to_le_bytes());
        hybrid[404..408].copy_from_slice(&0x1000u32.to_le_bytes());
        hybrid[408..412].copy_from_slice(&256u32.to_le_bytes());
        hybrid[412..416].copy_from_slice(&512u32.to_le_bytes());
        hybrid[512..516].copy_from_slice(&208u32.to_le_bytes());
        hybrid[712..720].copy_from_slice(&1u64.to_le_bytes());
        fs::write(&path, hybrid).unwrap();
        assert!(
            crate::detector::executable_guest(&path)
                .unwrap_err()
                .to_string()
                .contains("Unsupported ARM64EC/ARM64X")
        );
    }
}
