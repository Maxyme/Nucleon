//! Optional, game-local temporal upscaling. Providers remain experimental until game-tested.
mod assets;
mod install;
mod native;
mod ngx;
mod transaction;

use crate::{
    detector::{GraphicsApi, TargetEngine},
    launch::LaunchPlan,
    runtime::{ExecutionMode, FrontendAbi},
};
use anyhow::{Context, Result, ensure};
pub use assets::{Provider, install_patcher};
pub use install::install;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};
pub use transaction::{Binding, Installation, migrate_talos, prepare, preview, restore, status};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    #[default]
    Game,
    Fsr2,
    #[serde(rename = "fsr3.1")]
    Fsr31,
    Metalfx,
    Fsr4NativeLite,
    Fsr4Native,
    #[serde(rename = "fsr4.1.2")]
    Fsr412,
    Xess,
}
impl FromStr for Backend {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        serde_json::from_value(serde_json::Value::String(value.into())).with_context(||
            "Choose game, fsr2, fsr3.1, metalfx, fsr4-native-lite, fsr4-native, fsr4.1.2 or xess for upscaler")
    }
}
impl Backend {
    pub fn provider(self) -> &'static str {
        match self {
            Self::Metalfx | Self::Fsr4Native | Self::Fsr4NativeLite | Self::Fsr412 => "metalfx",
            _ => "stock",
        }
    }
    pub fn native(self) -> bool {
        self.native_model().is_some()
    }
    fn native_model(self) -> Option<native::Model> {
        match self {
            Self::Fsr4Native | Self::Fsr4NativeLite => Some(native::Model::Fsr411),
            Self::Fsr412 => Some(native::Model::Fsr412),
            _ => None,
        }
    }
    fn native_profile(self) -> Option<&'static str> {
        self.native_model().map(|model| {
            if self == Self::Fsr4Native {
                "exact-fast"
            } else {
                model.profile()
            }
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Input {
    #[default]
    Auto,
    Dlss,
    Fsr2,
    Fsr3,
    Fsr31,
    Xess,
}
impl FromStr for Input {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        serde_json::from_value(serde_json::Value::String(value.into()))
            .context("Choose auto, dlss, fsr2, fsr3, fsr31 or xess for upscaler input")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub backend: Backend,
    #[serde(default)]
    pub input: Input,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<PathBuf>,
}
impl Settings {
    pub fn selected(backend: Backend) -> Self {
        Self {
            backend,
            input: Input::Auto,
            renderer: None,
        }
    }
}

pub fn root() -> PathBuf {
    crate::paths::support_dir().join("upscalers")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolved {
    pub settings: Settings,
    pub executable: PathBuf,
    pub renderer: PathBuf,
    pub input: Input,
    pub api: String,
    pub runtime: PathBuf,
    pub provider: Option<PathBuf>,
    pub native_provider: Option<PathBuf>,
    pub bridge: Option<PathBuf>,
    pub patcher: bool,
    pub experimental: bool,
}

/// Resolve only: this never creates a directory, downloads, or executes Wine.
pub fn resolve(
    root: &Path,
    plan: &LaunchPlan,
    executable: &Path,
    settings: &Settings,
) -> Result<Resolved> {
    let executable = executable.canonicalize()?;
    let parent = executable.parent().context("Executable has no directory")?;
    let previous = transaction::lookup(root, &executable)?;
    let renderer = match &settings.renderer {
        Some(path) => parent
            .join(path)
            .canonicalize()
            .context("Cannot resolve upscaler rendering EXE")?,
        None => previous
            .as_ref()
            .map(|b| b.renderer.clone())
            .unwrap_or_else(|| executable.clone()),
    };
    ensure!(
        renderer.starts_with(parent),
        "Upscaler renderer must belong to the launcher's installation"
    );
    let mut resolved = Resolved {
        settings: settings.clone(),
        executable,
        renderer,
        input: settings.input,
        api: format!("{:?}", plan.api),
        runtime: plan.runtime.root.clone(),
        provider: None,
        native_provider: None,
        bridge: None,
        patcher: false,
        experimental: true,
    };
    if settings.backend == Backend::Game {
        return Ok(resolved);
    }
    if let Some(binding) = &previous
        && resolved.input == Input::Auto
    {
        resolved.input = binding.input;
    }
    ensure!(
        plan.is64
            && crate::detector::executable_guest(&resolved.renderer)?
                == crate::runtime::GuestAbi::X64,
        "Managed OptiScaler currently requires an x64 game; choose upscaler=game for x86"
    );
    ensure!(
        plan.runtime.graphics.frontend_abi != FrontendAbi::Arm64Ec,
        "OptiScaler with ARM64EC graphics is awaiting cross-ABI hook validation; choose upscaler=game"
    );
    let api = if resolved.renderer != resolved.executable && plan.api == GraphicsApi::Unknown {
        crate::detector::detect_target_engine(&resolved.renderer).api
    } else {
        plan.api.clone()
    };
    resolved.api = format!("{api:?}");
    ensure!(
        matches!(api, GraphicsApi::DirectX11 | GraphicsApi::DirectX12),
        "Managed upscaling requires DX11 or DX12; native Vulkan and older APIs remain gated"
    );
    let metalfx = settings.backend.provider() == "metalfx";
    let bundled = bundled_metalfx(&plan.runtime);
    if metalfx {
        ensure!(
            plan.runtime.mode == ExecutionMode::Intel
                && api == GraphicsApi::DirectX12
                && (plan.engine == TargetEngine::Gptk
                    || (plan.engine == TargetEngine::WineStaging && bundled)),
            "MetalFX/native FSR4 requires DX12 and a complete matching Intel GPTK runtime; no runtime is substituted"
        );
        let (framework, shared) = crate::runner::inspect_gptk_dir(&plan.runtime.root)
            .context("Missing matched D3DMetal framework")?;
        let bridge = crate::runner::gptk_windows_binaries_dir(&framework, &shared)
            .context("Missing GPTK Windows libraries")?
            .join("nvngx-on-metalfx.dll");
        ensure!(
            crate::runtime_checks::pe(Some(&bridge), true),
            "Selected GPTK has no x64 nvngx-on-metalfx.dll"
        );
        resolved.bridge = Some(bridge);
        if previous.as_ref().is_some_and(|b| {
            b.provider_version == "talos-migrated"
                && b.settings.backend == settings.backend
                && b.runtime == plan.runtime.root
        }) {
            return Ok(resolved);
        }
        if settings.backend.native() {
            let model = settings.backend.native_model().unwrap();
            let digest = crate::staging::digest(&framework.join("D3DMetal"))?;
            let manifest = root
                .join("providers")
                .join(model.provider())
                .join(assets::native_model_key(&digest, model)?)
                .join("provider.json");
            let provider = assets::read_provider(&manifest)?;
            ensure!(
                provider.provider == model.provider() && provider.version == model.version(),
                "Native provider identity/version differs from the selected model"
            );
            ensure!(
                provider.framework_sha256.as_deref()
                    == Some(crate::staging::digest(&framework.join("D3DMetal"))?.as_str()),
                "Prepared native hook belongs to a different GPTK framework; prepare it again"
            );
            resolved.native_provider = Some(manifest);
        }
    } else {
        ensure!(
            matches!(
                plan.engine,
                TargetEngine::Dxmt
                    | TargetEngine::Dxvk
                    | TargetEngine::Vkd3d
                    | TargetEngine::KosmicKrisp
                    | TargetEngine::Gptk
            ) || (plan.engine == TargetEngine::WineStaging && bundled),
            "Managed stock OptiScaler requires DXMT, DXVK, vkd3d or bundled GPTK; WineD3D is not validated"
        );
        ensure!(
            settings.backend != Backend::Xess || api == GraphicsApi::DirectX12,
            "XeSS is experimental on DX12 only; native DX11 XeSS requires Intel ARC"
        );
    }
    resolved.patcher = resolved
        .renderer
        .file_name()
        .is_some_and(|s| s.eq_ignore_ascii_case("Talos2-Win64-Shipping.exe"));
    if resolved.patcher && resolved.input == Input::Auto {
        resolved.input = Input::Dlss;
    }
    if resolved.input == Input::Auto {
        resolved.input = detect_input(resolved.renderer.parent().unwrap())?;
    }
    resolved.patcher &= resolved.input == Input::Dlss;
    ensure!(
        !metalfx || resolved.input == Input::Dlss,
        "The prepared MetalFX provider currently requires DLSS input"
    );
    ensure!(
        resolved.input != Input::Xess || !has_unreal(&resolved.renderer),
        "Unreal XeSS input may omit depth; choose a validated DLSS/FSR input recipe"
    );
    let provider = assets::provider_path(root, settings.backend.provider())?;
    ensure!(
        assets::read_provider(&provider)?.provider == settings.backend.provider(),
        "Provider identity differs from the selected output"
    );
    resolved.provider = Some(provider);
    if resolved.patcher {
        assets::verify_patcher(root)?;
    }
    Ok(resolved)
}

/// Complete vendor bundles can use an older Wine Unix-library naming scheme
/// (gdi32.dll.so rather than gdi32.so). Their own matched loader supplies the
/// ABI; the separate-runner export probe is not the capability of engine=wine.
fn bundled_metalfx(runtime: &crate::runtime::ResolvedRuntime) -> bool {
    if runtime.mode != ExecutionMode::Intel {
        return false;
    }
    let Some((framework, shared)) = crate::runner::inspect_gptk_dir(&runtime.root) else {
        return false;
    };
    let Some(windows) = crate::runner::gptk_windows_binaries_dir(&framework, &shared) else {
        return false;
    };
    let Ok(root) = runtime.root.canonicalize() else {
        return false;
    };
    [&framework.join("D3DMetal"), &shared].iter().all(|p| {
        p.canonicalize().is_ok_and(|p| p.starts_with(&root))
            && crate::runtime_checks::native_arch(p, object::Architecture::X86_64)
    }) && ["d3d11.dll", "dxgi.dll", "d3d12.dll", "nvngx-on-metalfx.dll"]
        .iter()
        .all(|name| crate::runtime_checks::pe(Some(&windows.join(name)), true))
}

fn has_unreal(exe: &Path) -> bool {
    exe.file_name().is_some_and(|n| {
        n.to_string_lossy()
            .to_ascii_lowercase()
            .contains("shipping")
    })
}
fn detect_input(directory: &Path) -> Result<Input> {
    let mut found = std::collections::BTreeSet::new();
    for entry in walkdir::WalkDir::new(directory)
        .max_depth(7)
        .follow_links(false)
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        match entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_str()
        {
            "nvngx_dlss.dll" => {
                found.insert("dlss");
            }
            "libxess.dll" => {
                found.insert("xess");
            }
            "amd_fidelityfx_upscaler_dx12.dll" => {
                found.insert("fsr31");
            }
            _ => {}
        }
    }
    ensure!(
        found.len() == 1,
        "Upscaler input is missing or ambiguous; prepare with --input dlss|fsr2|fsr31|xess and select that input in-game"
    );
    found.first().unwrap().parse()
}

pub(crate) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    crate::configuration::optional_json(path)?.with_context(|| {
        format!(
            "Missing {}; run 'nucleon upscaler install/prepare' first",
            path.display()
        )
    })
}
pub(crate) fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::create_dir_all(path.parent().context("No metadata parent")?)?;
    crate::fs_util::atomic_write_file(path, serde_json::to_vec_pretty(value)?)
}

/// Replace individual modules while preserving Steam and translator preferences.
pub fn compose_overrides(existing: &str, managed: &[(&str, &str)]) -> Result<String> {
    let mut modules = BTreeMap::new();
    for group in existing.split(';').filter(|g| !g.trim().is_empty()) {
        let (names, value) = group
            .split_once('=')
            .context("Malformed DLL override; use module=n,b")?;
        for name in names.split(',') {
            let name = name.trim().to_ascii_lowercase();
            ensure!(!name.is_empty(), "Empty DLL override module");
            if let Some(previous) = modules.insert(name.clone(), value.trim().to_string()) {
                ensure!(
                    previous == value.trim(),
                    "Conflicting DLL preferences for {name}"
                );
            }
        }
    }
    for (name, value) in managed {
        modules.insert((*name).into(), (*value).into());
    }
    Ok(modules
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(";"))
}

pub fn resolve_for_launch(
    root: &Path,
    plan: &LaunchPlan,
    executable: &Path,
    settings: &Settings,
) -> Result<Resolved> {
    if settings.backend != Backend::Game {
        ensure!(
            transaction::lookup(root, executable)?.is_some(),
            "No upscaler binding for this rendering EXE; run 'nucleon upscaler prepare <EXE> --backend <choice>' first. Installers are not injected automatically"
        );
    }
    resolve(root, plan, executable, settings)
}

pub fn validate_raw_overrides(raw: &str, backend: Backend) -> Result<()> {
    if backend == Backend::Game {
        return Ok(());
    }
    // The ordinary graphics preference can be replaced by our forwarding proxy;
    // contradictory user-supplied module preferences cannot be silently replaced.
    for group in raw.split(';').filter(|g| !g.trim().is_empty()) {
        let (names, value) = group
            .split_once('=')
            .context("Malformed NUCLEON_WINEDLLOVERRIDES")?;
        for name in names.split(',') {
            if name.trim().eq_ignore_ascii_case("dxgi")
                || (backend.provider() == "metalfx"
                    && ["nvngx", "nvapi64"]
                        .iter()
                        .any(|n| name.trim().eq_ignore_ascii_case(n)))
            {
                ensure!(
                    value.trim() == "n,b",
                    "NUCLEON_WINEDLLOVERRIDES conflicts with managed upscaler module {}",
                    name.trim()
                );
            }
        }
    }
    Ok(())
}

pub fn apply_environment(binding: &Binding, env: &mut HashMap<String, String>) -> Result<()> {
    // Never allow a stale native hook to survive an explicit provider switch.
    for key in env
        .keys()
        .filter(|k| k.starts_with("FSR4_"))
        .cloned()
        .collect::<Vec<_>>()
    {
        env.remove(&key);
    }
    env.remove("D3DMETAL_FRAMEWORK_PATH");
    if binding.settings.backend == Backend::Game {
        return Ok(());
    }
    for (key, value) in &binding.environment {
        env.insert(key.clone(), value.clone());
    }
    let mut overrides = vec![("dxgi", "n,b")];
    if binding.settings.backend.provider() == "metalfx" {
        overrides.extend([("nvngx", "n,b"), ("nvapi64", "n,b")]);
    }
    env.insert(
        "WINEDLLOVERRIDES".into(),
        compose_overrides(
            env.get("WINEDLLOVERRIDES")
                .map(String::as_str)
                .unwrap_or(""),
            &overrides,
        )?,
    );
    Ok(())
}

pub fn begin_observation(binding: &Binding) -> Result<()> {
    if let Some(directory) = binding.environment.get("FSR4_STATE_DIR") {
        let path = Path::new(directory).join("status.conf");
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}
pub fn observation(binding: &Binding) -> Result<serde_json::Value> {
    let Some(directory) = binding.environment.get("FSR4_STATE_DIR") else {
        return Ok(serde_json::json!({"activation":"not-observed"}));
    };
    let Some(text) =
        crate::configuration::optional_file(&Path::new(directory).join("status.conf"))?
    else {
        return Ok(serde_json::json!({"activation":"not-observed"}));
    };
    let fields: BTreeMap<_, _> = text.lines().filter_map(|l| l.split_once('=')).collect();
    let count = |key| fields.get(key).and_then(|s| s.parse::<u64>().ok());
    let encodes = count("encodes").unwrap_or(0);
    if encodes == 0 {
        return Ok(serde_json::json!({"activation":"not-observed"}));
    }
    let active = fields.get("active") == Some(&"1") && count("replaced").unwrap_or(0) > 0;
    let size = |width, height| -> Option<[u64; 2]> {
        let (width, height) = (count(width)?, count(height)?);
        (width > 0 && height > 0).then_some([width, height])
    };
    // Upstream's frame_ms measures CPU submission cadence. It is neither
    // scaler GPU duration nor measured presentation FPS.
    let submission_interval_ms = fields
        .get("frame_ms")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|ms| ms.is_finite() && *ms > 0.0);
    Ok(
        serde_json::json!({"activation":if active {"native-status-active"} else {"metalfx-fallback"},
        "version":fields.get("fsr_version"),"profile":fields.get("profile"),"reason":fields.get("reason"),"pid":fields.get("pid"),
        "encodes":encodes,"replaced":count("replaced"),"stubs":count("stubs"),
        "model":fields.get("model"),"preset":fields.get("preset"),"variant":fields.get("variant"),
        "render_size":size("rw","rh"),"output_size":size("uw","uh"),"aligned_output_size":size("uwA","uhA"),
        "reported_at_unix_seconds":count("t"),"submission_interval_ms":submission_interval_ms}),
    )
}

#[cfg(test)]
mod tests;
