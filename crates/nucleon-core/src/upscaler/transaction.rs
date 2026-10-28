use super::{Backend, Input, Resolved, Settings, assets, read, save};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};

fn write_owned(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().context("Owned file has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| anyhow::anyhow!("Cannot publish {}: {}", path.display(), error.error))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Fingerprint {
    sha256: Option<String>,
    mode: Option<u32>,
    symlink: Option<PathBuf>,
}
fn fingerprint(path: &Path) -> Result<Option<Fingerprint>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if meta.is_symlink() {
        return Ok(Some(Fingerprint {
            sha256: None,
            mode: None,
            symlink: Some(fs::read_link(path)?),
        }));
    }
    ensure!(meta.is_file(), "Expected a file: {}", path.display());
    Ok(Some(Fingerprint {
        sha256: Some(crate::staging::digest(path)?),
        mode: Some(meta.permissions().mode() & 0o777),
        symlink: None,
    }))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileRecord {
    relative: PathBuf,
    original: Option<Fingerprint>,
    backup: PathBuf,
    installed: Option<Fingerprint>,
    #[serde(default)]
    previous: Option<Fingerprint>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Journal {
    schema: u32,
    status: String,
    directory: PathBuf,
    files: Vec<FileRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    pub schema: u32,
    pub executable: PathBuf,
    pub renderer: PathBuf,
    pub runtime: PathBuf,
    pub settings: Settings,
    pub input: Input,
    pub api: String,
    pub provider_version: String,
    pub provider: Option<PathBuf>,
    pub native_provider: Option<PathBuf>,
    pub environment: BTreeMap<String, String>,
    pub support_files: BTreeMap<PathBuf, String>,
    pub experimental: bool,
    pub activation: String,
}
pub struct Installation {
    pub state: PathBuf,
    pub directory: PathBuf,
    _lock: Option<fs::File>,
}
pub(crate) fn lock(path: &Path) -> Result<fs::File> {
    ensure!(!path.is_symlink(), "Lock must not be a symlink");
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .context("Another command or running game owns this upscaler installation")?;
    Ok(file)
}
fn identity(path: &Path) -> Result<String> {
    crate::staging::key(&path)
}
fn index(root: &Path, exe: &Path) -> Result<PathBuf> {
    Ok(root
        .join("bindings")
        .join(format!("{}.json", identity(&exe.canonicalize()?)?)))
}
pub(crate) fn lookup(root: &Path, exe: &Path) -> Result<Option<Binding>> {
    let pointer: Option<PathBuf> = crate::configuration::optional_json(&index(root, exe)?)?;
    pointer
        .map(|state| {
            ensure!(
                state.starts_with(root.join("games")),
                "Binding state escapes the upscaler registry"
            );
            let current = state.join("binding.json");
            let pending = state.join("pending-binding.json");
            let binding: Binding = read(if current.exists() { &current } else { &pending })?;
            ensure!(
                binding.schema == 1
                    && state
                        == root.join("games").join(identity(
                            binding
                                .renderer
                                .parent()
                                .context("Invalid renderer owner")?
                        )?),
                "Binding state owner differs from its renderer"
            );
            Ok(binding)
        })
        .transpose()
}
impl Installation {
    pub fn open(root: &Path, resolved: &Resolved) -> Result<Self> {
        let directory = resolved
            .renderer
            .parent()
            .context("No renderer directory")?
            .to_path_buf();
        let state = root.join("games").join(identity(&directory)?);
        ensure!(
            !state.is_symlink(),
            "Upscaler state directory must not be a symlink"
        );
        fs::create_dir_all(&state)?;
        let file = lock(&state.join(".installation.lock"))?;
        Ok(Self {
            state,
            directory,
            _lock: Some(file),
        })
    }
    pub fn existing(root: &Path, exe: &Path) -> Result<Option<Self>> {
        let Some(binding) = lookup(root, exe)? else {
            return Ok(None);
        };
        let resolved = Resolved {
            settings: binding.settings,
            executable: binding.executable,
            renderer: binding.renderer,
            input: binding.input,
            api: binding.api,
            runtime: binding.runtime,
            provider: binding.provider,
            native_provider: binding.native_provider,
            bridge: None,
            patcher: false,
            experimental: true,
        };
        Self::open(root, &resolved).map(Some)
    }
    pub fn assert_idle(&self, prefix: Option<&Path>) -> Result<()> {
        let commands = std::process::Command::new("/bin/ps")
            .args(["-axo", "pid=,comm="])
            .output()?;
        ensure!(
            commands.status.success(),
            "Cannot inspect process identities"
        );
        let wine_pids: std::collections::BTreeSet<_> = String::from_utf8_lossy(&commands.stdout)
            .lines()
            .filter_map(|line| {
                let (pid, command) = line.trim().split_once(char::is_whitespace)?;
                let pid = pid.parse::<u32>().ok()?;
                (pid != std::process::id() && wine_command(command.trim())).then_some(pid)
            })
            .collect();
        let output = std::process::Command::new("/bin/ps")
            .args(["eww", "-axo", "pid=,args="])
            .output()?;
        ensure!(
            output.status.success(),
            "Cannot inspect active game processes"
        );
        let rows = String::from_utf8_lossy(&output.stdout);
        ensure!(
            !rows
                .lines()
                .filter(|row| row
                    .trim()
                    .split_once(char::is_whitespace)
                    .and_then(|(pid, _)| pid.parse::<u32>().ok())
                    .is_some_and(|pid| wine_pids.contains(&pid)))
                .any(
                    |row| row.contains(self.directory.to_string_lossy().as_ref())
                        || prefix
                            .is_some_and(|p| row.contains(&format!("WINEPREFIX={}", p.display())))
                ),
            "Game installation or prefix is in use; quit it before preparing/restoring upscaling"
        );
        Ok(())
    }
    fn target(&self, relative: &Path) -> Result<PathBuf> {
        ensure!(
            !relative.as_os_str().is_empty()
                && !relative.is_absolute()
                && relative
                    .components()
                    .all(|p| matches!(p, std::path::Component::Normal(_))),
            "Unsafe game-local target"
        );
        let target = self.directory.join(relative);
        let mut parent = target.parent().unwrap();
        while parent != self.directory {
            ensure!(
                !parent.is_symlink(),
                "Game-local parent is a symlink: {}",
                parent.display()
            );
            parent = parent.parent().context("Target escaped installation")?;
        }
        Ok(target)
    }
    fn validate(&self, journal: &Journal, allow_mutable: bool) -> Result<Vec<PathBuf>> {
        ensure!(
            journal.schema == 1 && journal.directory == self.directory,
            "Invalid game-local journal owner"
        );
        let mut mutable = vec![];
        for entry in &journal.files {
            let current = fingerprint(&self.target(&entry.relative)?)?;
            if current != entry.installed && current != entry.original && current != entry.previous
            {
                if allow_mutable
                    && entry.relative == Path::new("OptiScaler.ini")
                    && current.as_ref().is_some_and(|f| f.symlink.is_none())
                {
                    mutable.push(entry.relative.clone());
                } else {
                    bail!(
                        "Managed file changed: {}. Preserve the edit before restoring",
                        entry.relative.display()
                    );
                }
            }
            if entry.original.is_some() {
                ensure!(
                    fingerprint(&entry.backup)? == entry.original,
                    "Missing or changed original backup"
                );
            }
        }
        Ok(mutable)
    }
    fn archive_settings(&self, mutable: &[PathBuf]) -> Result<()> {
        for relative in mutable {
            let directory = self.state.join("user-settings");
            fs::create_dir_all(&directory)?;
            let file = directory.join(format!(
                "{}-OptiScaler.ini",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
            fs::copy(self.target(relative)?, file)?;
        }
        Ok(())
    }
    fn preflight(&self, desired: &BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        let prior: Option<Journal> =
            crate::configuration::optional_json(&self.state.join("install.json"))?;
        if let Some(journal) = &prior {
            ensure!(
                journal.status != "installing",
                "Interrupted preparation; restore it before retrying"
            );
            self.validate(journal, true)?;
            if journal.status == "prepared" {
                for e in &journal.files {
                    if e.relative != Path::new("OptiScaler.ini") {
                        ensure!(
                            fingerprint(&self.target(&e.relative)?)? == e.installed,
                            "Prepared file changed: {}",
                            e.relative.display()
                        );
                    }
                }
            }
        }
        for relative in desired.keys() {
            if prior
                .as_ref()
                .is_none_or(|j| !j.files.iter().any(|e| e.relative == *relative))
            {
                let target = self.target(relative)?;
                ensure!(
                    fingerprint(&target)?.is_none(),
                    "Existing unowned file {}; refusing to overwrite it",
                    target.display()
                );
            }
        }
        Ok(())
    }
    fn reconcile(&self, desired: &BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        let path = self.state.join("install.json");
        let prior: Option<Journal> = crate::configuration::optional_json(&path)?;
        let mut journal = prior.unwrap_or(Journal {
            schema: 1,
            status: "restored".into(),
            directory: self.directory.clone(),
            files: vec![],
        });
        ensure!(
            journal.status != "installing",
            "Interrupted upscaler preparation; run 'nucleon upscaler restore' first"
        );
        let mutable = self.validate(&journal, true)?;
        // A prepared installation must match every active executable exactly.
        if journal.status == "prepared" {
            for e in &journal.files {
                if e.relative != Path::new("OptiScaler.ini") {
                    ensure!(
                        fingerprint(&self.target(&e.relative)?)? == e.installed,
                        "Prepared file is missing/changed: {}",
                        e.relative.display()
                    );
                }
            }
        }
        for relative in desired.keys() {
            if !journal.files.iter().any(|e| e.relative == *relative) {
                let target = self.target(relative)?;
                ensure!(
                    fingerprint(&target)?.is_none(),
                    "Existing unowned file {}; refusing to replace a mod or invent a DLL chain",
                    target.display()
                );
                journal.files.push(FileRecord {
                    relative: relative.clone(),
                    original: None,
                    backup: self.state.join("backups").join(relative),
                    installed: None,
                    previous: None,
                });
            }
        }
        self.archive_settings(&mutable)?;
        for e in &mut journal.files {
            e.previous = fingerprint(&self.target(&e.relative)?)?;
            e.installed = if let Some(data) = desired.get(&e.relative) {
                use sha2::{Digest, Sha256};
                Some(Fingerprint {
                    sha256: Some(format!("{:x}", Sha256::digest(data))),
                    mode: Some(0o644),
                    symlink: None,
                })
            } else {
                e.original.clone()
            };
        }
        journal.status = "installing".into();
        save(&path, &journal)?;
        let result = (|| -> Result<()> {
            for e in &journal.files {
                let target = self.target(&e.relative)?;
                if fingerprint(&target)? == e.installed {
                    continue;
                }
                if let Some(data) = desired.get(&e.relative) {
                    fs::create_dir_all(target.parent().unwrap())?;
                    write_owned(&target, data, 0o644)?;
                } else {
                    restore_entry(&target, e)?;
                }
            }
            journal.status = "prepared".into();
            save(&path, &journal)
        })();
        if let Err(error) = result {
            restore(self)
                .with_context(|| format!("Preparation failed ({error:#}); rollback also failed"))?;
            return Err(error);
        }
        Ok(())
    }
}
fn restore_entry(target: &Path, e: &FileRecord) -> Result<()> {
    if fingerprint(target)? == e.original {
        return Ok(());
    }
    if let Some(original) = &e.original {
        if let Some(link) = &original.symlink {
            if fs::symlink_metadata(target).is_ok() {
                fs::remove_file(target)?;
            }
            symlink(link, target)?;
        } else {
            write_owned(
                target,
                &fs::read(&e.backup)?,
                original.mode.context("Missing original permissions")?,
            )?;
        }
    } else if fs::symlink_metadata(target).is_ok() {
        fs::remove_file(target)?;
    }
    Ok(())
}
pub(super) fn wine_command(command: &str) -> bool {
    let name = Path::new(command)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "wine" | "wine64" | "wineserver" | "wineserver64" | "wine-preloader" | "wine64-preloader"
    ) || name.ends_with(".exe")
}
pub fn restore(installation: &Installation) -> Result<()> {
    let path = installation.state.join("install.json");
    let Some(mut journal) = crate::configuration::optional_json::<Journal>(&path)? else {
        return Ok(());
    };
    let mutable = installation.validate(&journal, true)?;
    installation.archive_settings(&mutable)?;
    for e in journal.files.iter().rev() {
        restore_entry(&installation.target(&e.relative)?, e)?;
    }
    journal.status = "restored".into();
    save(&path, &journal)
}
pub fn status(root: &Path, exe: &Path) -> Result<serde_json::Value> {
    let Some(binding) = lookup(root, exe)? else {
        return Ok(if legacy_conflict(root, &exe.canonicalize()?)? {
            serde_json::json!({"status":"legacy-prepared","activation":"not-observed","migration_required":true,"next":"nucleon upscaler migrate-talos"})
        } else {
            serde_json::json!({"status":"unmanaged","activation":"not-observed"})
        });
    };
    let state = root
        .join("games")
        .join(identity(binding.renderer.parent().unwrap())?);
    let journal: Journal = read(&state.join("install.json"))?;
    let mut changed = vec![];
    let mut settings_modified = false;
    for e in &journal.files {
        let target = journal.directory.join(&e.relative);
        let expected = if journal.status == "restored" {
            &e.original
        } else {
            &e.installed
        };
        if fingerprint(&target)? != *expected {
            if e.relative == Path::new("OptiScaler.ini") {
                settings_modified = true;
            } else {
                changed.push(target);
            }
        }
    }
    for (path, expected) in &binding.support_files {
        if !path.is_file() || crate::staging::digest(path)? != *expected {
            changed.push(path.clone());
        }
    }
    let observed = super::observation(&binding)?;
    Ok(
        serde_json::json!({"status":journal.status,"binding":binding,"changed_files":changed,
        "settings_modified":settings_modified,"activation":"not-observed","last_native_report":observed,"state":state}),
    )
}
fn windows(path: &Path) -> String {
    format!("Z:{}", path.display().to_string().replace('/', "\\"))
}
pub fn preview(root: &Path, resolved: &Resolved) -> Result<()> {
    ensure!(
        !legacy_conflict(root, &resolved.renderer)?,
        "Existing Talos preparation requires 'nucleon upscaler migrate-talos'"
    );
    let directory = resolved.renderer.parent().unwrap().to_path_buf();
    let installation = Installation {
        state: root.join("games").join(identity(&directory)?),
        directory,
        _lock: None,
    };
    if resolved.settings.backend == Backend::Game {
        if let Some(journal) = crate::configuration::optional_json::<Journal>(
            &installation.state.join("install.json"),
        )? {
            installation.validate(&journal, true)?;
        }
        return Ok(());
    }
    if let Some(manifest) = &resolved.provider {
        let provider = assets::read_provider(manifest)?;
        let mut desired = BTreeMap::from([
            (PathBuf::from("dxgi.dll"), vec![]),
            (PathBuf::from("OptiScaler.ini"), vec![]),
        ]);
        for relative in provider.files.keys() {
            if relative
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
                && relative != Path::new("OptiScaler.dll")
            {
                desired.insert(PathBuf::from("OptiScaler").join(relative), vec![]);
            }
        }
        if resolved.bridge.is_some() {
            desired.insert("OptiScaler/nucleon-metalfx/nvngx.dll".into(), vec![]);
        }
        if resolved.patcher {
            desired.insert("OptiScaler/plugins/OptiPatcher.asi".into(), vec![]);
        }
        installation.preflight(&desired)?;
    }
    Ok(())
}
fn ini(template: &str, changes: &BTreeMap<(String, String), String>) -> Result<Vec<u8>> {
    let mut section = "";
    let mut remaining = changes.clone();
    let mut output = String::new();
    for line in template.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = &trimmed[1..trimmed.len() - 1];
        }
        if !trimmed.starts_with(';')
            && let Some((key, _)) = trimmed.split_once('=')
            && let Some(value) = remaining.remove(&(section.into(), key.trim().into()))
        {
            output.push_str(&format!("{}={value}\n", key.trim()));
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    ensure!(
        remaining.is_empty(),
        "Pinned provider INI lacks required settings: {:?}",
        remaining.keys()
    );
    Ok(output.into_bytes())
}
pub fn prepare(root: &Path, installation: &Installation, resolved: &Resolved) -> Result<Binding> {
    ensure!(
        !legacy_conflict(root, &resolved.renderer)?,
        "Talos has an existing preparation journal. Run 'nucleon upscaler migrate-talos' before changing it"
    );
    if resolved.provider.is_none() && resolved.settings.backend != Backend::Game {
        let binding: Binding = read(&installation.state.join("binding.json"))?;
        ensure!(
            binding.provider_version == "talos-migrated"
                && binding.settings.backend == resolved.settings.backend
                && binding.runtime == resolved.runtime,
            "Install and prepare the selected provider before switching this migrated binding"
        );
        let report = status(root, &resolved.executable)?;
        ensure!(
            report["status"] == "prepared"
                && report["changed_files"]
                    .as_array()
                    .is_some_and(|a| a.is_empty()),
            "Migrated Talos preparation changed; inspect status"
        );
        return Ok(binding);
    }
    if resolved.settings.backend == Backend::Game {
        restore(installation)?;
        return Ok(Binding {
            schema: 1,
            executable: resolved.executable.clone(),
            renderer: resolved.renderer.clone(),
            runtime: resolved.runtime.clone(),
            settings: resolved.settings.clone(),
            input: resolved.input,
            api: resolved.api.clone(),
            provider_version: "game".into(),
            provider: None,
            native_provider: None,
            environment: BTreeMap::new(),
            support_files: BTreeMap::new(),
            experimental: false,
            activation: "not-observed".into(),
        });
    }
    if let Some(binding) = lookup(root, &resolved.executable)?
        && binding.settings.backend == resolved.settings.backend
        && binding.input == resolved.input
        && binding.renderer == resolved.renderer
        && binding.runtime == resolved.runtime
        && binding.provider == resolved.provider
        && binding.native_provider == resolved.native_provider
        && installation.state.join("install.json").is_file()
    {
        let report = status(root, &resolved.executable)?;
        ensure!(
            report["changed_files"]
                .as_array()
                .is_some_and(|a| a.is_empty()),
            "Prepared upscaler files changed; inspect status/restore before retrying"
        );
        if report["status"] == "prepared" {
            return Ok(binding);
        }
    }
    let manifest = resolved
        .provider
        .as_ref()
        .context("Missing resolved provider")?;
    let provider = assets::read_provider(manifest)?;
    let provider_dir = manifest.parent().unwrap();
    let mut files = BTreeMap::new();
    let mut support = BTreeMap::new();
    for (relative, digest) in &provider.files {
        support.insert(provider_dir.join(relative), digest.clone());
        if relative
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
            && relative != Path::new("OptiScaler.dll")
        {
            files.insert(
                PathBuf::from("OptiScaler").join(relative),
                fs::read(provider_dir.join(relative))?,
            );
        }
    }
    files.insert(
        "dxgi.dll".into(),
        fs::read(provider_dir.join("OptiScaler.dll"))?,
    );
    fs::create_dir_all(installation.state.join("logs"))?;
    let mut env = BTreeMap::new();
    let mut changes = BTreeMap::new();
    let output = match resolved.settings.backend {
        Backend::Fsr2 => "fsr22",
        Backend::Fsr31 => "fsr31",
        Backend::Xess => "xess",
        _ => "dlss",
    };
    let key = if resolved.api == "DirectX11" {
        "Dx11Upscaler"
    } else {
        "Dx12Upscaler"
    };
    for (section, key, value) in [
        ("Upscalers", key, output.to_string()),
        ("FrameGen", "Enabled", "false".into()),
        ("FrameGen", "FGInput", "nofg".into()),
        ("FrameGen", "FGOutput", "nofg".into()),
        // Stock outputs need no NVIDIA driver probe. The MetalFX fork enables
        // original NGX itself when passthrough is requested below.
        ("DLSS", "Enabled", "false".into()),
        (
            "Inputs",
            "EnableDlssInputs",
            (resolved.input == Input::Dlss).to_string(),
        ),
        (
            "Inputs",
            "EnableXeSSInputs",
            (resolved.input == Input::Xess).to_string(),
        ),
        (
            "Inputs",
            "EnableFsr2Inputs",
            (resolved.input == Input::Fsr2).to_string(),
        ),
        (
            "Inputs",
            "EnableFsr3Inputs",
            (resolved.input == Input::Fsr3).to_string(),
        ),
        (
            "Inputs",
            "EnableFfxInputs",
            (resolved.input == Input::Fsr31).to_string(),
        ),
        (
            "Libraries",
            "OptiDllPath",
            windows(&installation.directory.join("OptiScaler")),
        ),
        ("Spoofing", "Dxgi", "false".into()),
        ("Spoofing", "Vulkan", "false".into()),
        ("Spoofing", "StreamlineSpoofing", "false".into()),
        ("Spoofing", "Registry", "false".into()),
        ("NvApi", "OverrideNvapiDll", "false".into()),
        ("Menu", "OverlayMenu", "false".into()),
        ("Plugins", "LoadAsiPlugins", resolved.patcher.to_string()),
        (
            "Plugins",
            "Path",
            windows(&installation.directory.join("OptiScaler/plugins")),
        ),
        ("Log", "LogToFile", "true".into()),
        ("Log", "LogToConsole", "false".into()),
        ("Log", "LogLevel", "2".into()),
        (
            "Log",
            "LogFileName",
            windows(&installation.state.join("logs/OptiScaler.log")),
        ),
    ] {
        changes.insert((section.into(), key.into()), value);
    }
    if let Some(bridge) = &resolved.bridge {
        let manifest = assets::bridge_provider(root, bridge)?;
        let provider = assets::read_provider(&manifest)?;
        let directory = manifest.parent().unwrap();
        support.extend(
            provider
                .files
                .iter()
                .map(|(path, hash)| (directory.join(path), hash.clone())),
        );
        changes.insert(("DLSS".into(), "PassThrough".into()), "true".into());
        changes.insert(
            ("Libraries".into(), "NvngxPath".into()),
            windows(&installation.directory.join("OptiScaler/nucleon-metalfx")),
        );
        files.insert(
            "OptiScaler/nucleon-metalfx/nvngx.dll".into(),
            fs::read(directory.join("nvngx.dll"))?,
        );
        // The vendor bridge is filename-sensitive under Wine. A real PE
        // forwarder supplies the expected game name without renaming it.
        // Static NGX accepts Wine's builtin PE entry before LoadLibrary is
        // intercepted by OptiScaler. Actual calls use the forwarder in
        // NvngxPath, preserving the real bridge's original module name.
        files.insert("nvngx.dll".into(), fs::read(bridge)?);
        files.insert("nvngx-on-metalfx.dll".into(), fs::read(bridge)?);
        support.insert(bridge.clone(), crate::staging::digest(bridge)?);
        env.insert("D3DM_ENABLE_METALFX".into(), "1".into());
    }
    if resolved.patcher {
        let patcher = assets::verify_patcher(root)?;
        files.insert(
            "OptiScaler/plugins/OptiPatcher.asi".into(),
            fs::read(&patcher)?,
        );
        support.insert(patcher.clone(), crate::staging::digest(&patcher)?);
    }
    if let Some(native) = &resolved.native_provider {
        let native_root = native.parent().unwrap();
        let native_provider = assets::read_provider(native)?;
        support.extend(
            native_provider
                .files
                .iter()
                .map(|(p, h)| (native_root.join(p), h.clone())),
        );
        let source = native_root.join("fsr4");
        for (key, value) in [
            (
                "D3DMETAL_FRAMEWORK_PATH",
                source
                    .join("dist/crossover/D3DMetal.framework/Versions/A/D3DMetal")
                    .display()
                    .to_string(),
            ),
            ("FSR4_ROOT", source.display().to_string()),
            ("FSR4_REPLACE", "1".into()),
            (
                "FSR4_VERSION",
                resolved
                    .settings
                    .backend
                    .native_model()
                    .unwrap()
                    .version()
                    .into(),
            ),
            (
                "FSR4_PROFILE",
                resolved.settings.backend.native_profile().unwrap().into(),
            ),
            (
                "FSR4_SETTINGS",
                installation.state.join("panel.conf").display().to_string(),
            ),
            ("FSR4_STATE_DIR", installation.state.display().to_string()),
            (
                "FSR4_HOOK_LOG",
                installation
                    .state
                    .join("logs/fsr4-hook.log")
                    .display()
                    .to_string(),
            ),
            ("FSR4_PANEL", "1".into()),
        ] {
            env.insert(key.into(), value);
        }
        // A launch-selected mode wins over a previous panel selection.
        crate::fs_util::atomic_write_file(
            installation.state.join("panel.conf"),
            format!("sharpness=0.400\nrcas=1\nprofile={}\n", env["FSR4_PROFILE"]),
        )?;
    }
    files.insert(
        "OptiScaler.ini".into(),
        ini(
            &fs::read_to_string(provider_dir.join("OptiScaler.ini"))?,
            &changes,
        )?,
    );
    installation.preflight(&files)?;
    let binding = Binding {
        schema: 1,
        executable: resolved.executable.clone(),
        renderer: resolved.renderer.clone(),
        runtime: resolved.runtime.clone(),
        settings: resolved.settings.clone(),
        input: resolved.input,
        api: resolved.api.clone(),
        provider_version: provider.version,
        provider: resolved.provider.clone(),
        native_provider: resolved.native_provider.clone(),
        environment: env,
        support_files: support,
        experimental: true,
        activation: "not-observed".into(),
    };
    // Publish the binding and alias before writes so interrupted first installs can be restored.
    save(&installation.state.join("pending-binding.json"), &binding)?;
    save(&index(root, &binding.executable)?, &installation.state)?;
    save(&index(root, &binding.renderer)?, &installation.state)?;
    installation.reconcile(&files)?;
    save(&installation.state.join("binding.json"), &binding)?;
    fs::remove_file(installation.state.join("pending-binding.json"))?;
    Ok(binding)
}

fn legacy_conflict(root: &Path, renderer: &Path) -> Result<bool> {
    let state = root.join("talos2-fsr4-lite");
    let profile: Option<serde_json::Value> =
        crate::configuration::optional_json(&state.join("profile.json"))?;
    if let Some(p) = profile
        && (p["renderer"]
            .as_str()
            .is_some_and(|v| Path::new(v) == renderer)
            || p["game"]
                .as_str()
                .is_some_and(|v| Path::new(v).join("Talos2.exe") == renderer))
    {
        let receipt: serde_json::Value = read(&state.join("install.json"))?;
        return Ok(receipt["status"] == "prepared");
    }
    Ok(false)
}

/// Explicitly transfer ownership while keeping the original Talos backups intact.
pub fn migrate_talos(root: &Path) -> Result<()> {
    migrate_talos_checked(root, |installation, prefix| {
        installation.assert_idle(Some(prefix))
    })
}
pub(super) fn migrate_talos_checked(
    root: &Path,
    check_idle: impl Fn(&Installation, &Path) -> Result<()>,
) -> Result<()> {
    let old = root.join("talos2-fsr4-lite");
    let profile: serde_json::Value = read(&old.join("profile.json"))?;
    let mut receipt: serde_json::Value = read(&old.join("install.json"))?;
    ensure!(
        receipt["status"] == "prepared",
        "Talos preparation is not in prepared state"
    );
    let game = PathBuf::from(
        profile["game"]
            .as_str()
            .context("Missing Talos game root")?,
    );
    let renderer = PathBuf::from(
        profile["renderer"]
            .as_str()
            .context("Missing Talos renderer")?,
    );
    let exe = game.join("Talos2.exe").canonicalize()?;
    ensure!(
        renderer.canonicalize()?
            == game
                .join("Talos2/Binaries/Win64/Talos2-Win64-Shipping.exe")
                .canonicalize()?,
        "Unexpected Talos renderer"
    );
    let resolved = Resolved {
        settings: Settings {
            backend: Backend::Fsr4NativeLite,
            input: Input::Dlss,
            renderer: Some(renderer.strip_prefix(&game)?.to_path_buf()),
        },
        executable: exe.clone(),
        renderer: renderer.clone(),
        input: Input::Dlss,
        api: "DirectX12".into(),
        runtime: PathBuf::from(profile["wine"].as_str().context("Missing runtime")?),
        provider: None,
        native_provider: None,
        bridge: None,
        patcher: true,
        experimental: true,
    };
    let installation = Installation::open(root, &resolved)?;
    check_idle(
        &installation,
        Path::new(profile["prefix"].as_str().context("Missing prefix")?),
    )?;
    let _old_lock = lock(&old.join("setup.lock"))?;
    let prefix = PathBuf::from(profile["prefix"].as_str().context("Missing prefix")?);
    let _prefix_lock = lock(&prefix.join(".nucleon-launch.lock"))?;
    let mut journal = Journal {
        schema: 1,
        status: "prepared".into(),
        directory: renderer.parent().unwrap().to_path_buf(),
        files: vec![],
    };
    for entry in receipt["files"]
        .as_array()
        .context("Missing Talos file records")?
    {
        let target = game.join(entry["relative"].as_str().context("Missing target")?);
        let relative = target.strip_prefix(&journal.directory)?.to_path_buf();
        installation.target(&relative)?;
        let installed: Option<Fingerprint> = serde_json::from_value(entry["installed"].clone())?;
        let original: Option<Fingerprint> = serde_json::from_value(entry["original"].clone())?;
        ensure!(
            fingerprint(&target)? == installed,
            "Talos file changed; preserve it before migrating"
        );
        let backup = PathBuf::from(entry["backup"].as_str().context("Missing backup")?);
        if original.is_some() {
            ensure!(fingerprint(&backup)? == original, "Talos backup changed");
        }
        journal.files.push(FileRecord {
            relative,
            original,
            backup,
            installed,
            previous: None,
        });
    }
    let mut support_files: BTreeMap<PathBuf, String> =
        serde_json::from_value(profile["support_files"].clone())?;
    let framework = resolved
        .runtime
        .join("lib/external/D3DMetal.framework/D3DMetal");
    let original_digest = profile["framework_original_sha256"]
        .as_str()
        .context("Missing original GPTK fingerprint")?;
    ensure!(
        crate::staging::digest(&framework)? == original_digest,
        "Original GPTK framework changed"
    );
    support_files.insert(framework, original_digest.into());
    for (p, h) in &support_files {
        ensure!(
            crate::staging::digest(p)? == *h,
            "Talos support file changed"
        );
    }
    let environment: BTreeMap<String, String> =
        serde_json::from_value(profile["environment"].clone())?;
    let binding = Binding {
        schema: 1,
        executable: exe,
        renderer,
        runtime: resolved.runtime,
        settings: resolved.settings,
        input: Input::Dlss,
        api: resolved.api,
        provider_version: "talos-migrated".into(),
        provider: None,
        native_provider: None,
        environment: environment
            .into_iter()
            .filter(|(k, _)| !k.starts_with("NUCLEON_"))
            .collect(),
        support_files,
        experimental: true,
        activation: "not-observed".into(),
    };
    if let Some(existing) =
        crate::configuration::optional_json::<Journal>(&installation.state.join("install.json"))?
    {
        ensure!(
            existing == journal,
            "A different generic preparation already owns this installation"
        );
        let existing: Binding = read(&installation.state.join("binding.json"))?;
        ensure!(
            existing.provider_version == "talos-migrated"
                && existing.executable == binding.executable
                && existing.support_files == binding.support_files,
            "Cannot resume migration over a different preparation"
        );
    }
    save(&installation.state.join("binding.json"), &binding)?;
    save(&installation.state.join("install.json"), &journal)?;
    save(&index(root, &binding.executable)?, &installation.state)?;
    save(&index(root, &binding.renderer)?, &installation.state)?;
    receipt["status"] = "migrated".into();
    receipt["migrated_to"] = installation.state.display().to_string().into();
    save(&old.join("install.json"), &receipt)?;
    Ok(())
}
