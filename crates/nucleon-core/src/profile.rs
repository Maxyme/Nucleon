//! Structured per-game choices shared by Steam launches and diagnostic wrappers,
//! backed by `assets/game-configs.toml`.
use crate::repo_assets::ASSETS;
use crate::{
    detector::GraphicsApi,
    runtime::{ResolvedRuntime, SteamProtocol},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsPreference {
    Auto,
    Dx7,
    Dx9,
    Dx10,
    Dx11,
    Dx12,
    Vulkan,
    Opengl,
}

impl GraphicsPreference {
    pub fn api(self) -> Option<GraphicsApi> {
        Some(match self {
            Self::Auto => return None,
            Self::Dx7 => GraphicsApi::DirectX7OrOlder,
            Self::Dx9 => GraphicsApi::DirectX9OrOlder,
            Self::Dx10 => GraphicsApi::DirectX10,
            Self::Dx11 => GraphicsApi::DirectX11,
            Self::Dx12 => GraphicsApi::DirectX12,
            Self::Vulkan => GraphicsApi::Vulkan,
            Self::Opengl => GraphicsApi::OpenGL,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upscaler: Option<crate::upscaler::Settings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_generation: Option<crate::frame_generation::Setting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hud: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_steam_shim: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dxvk_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vulkan_icd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emulator: Option<crate::runtime_selection::Emulator>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graphics_api: Option<GraphicsPreference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steam_protocol: Option<SteamProtocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steam_bridge: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpscalerGameConfigs {
    #[serde(default)]
    pub dlss_patcher_executables: Vec<String>,
    #[serde(default)]
    pub input_dlls: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GraphicsApiConfigRule {
    pub executable: String,
    pub config_file: String,
    pub section: String,
    pub key: String,
    pub values: BTreeMap<String, GraphicsPreference>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GameConfigs {
    pub schema: u32,
    #[serde(default)]
    pub upscaler: UpscalerGameConfigs,
    #[serde(default)]
    pub graphics_api_configs: Vec<GraphicsApiConfigRule>,
    #[serde(default)]
    pub fex_profiles: BTreeMap<String, GameProfile>,
}

pub(crate) fn game_configs() -> &'static GameConfigs {
    static CONFIGS: OnceLock<GameConfigs> = OnceLock::new();
    CONFIGS.get_or_init(|| {
        let configs: GameConfigs = toml::from_str(
            ASSETS
                .get_file("game-configs.toml")
                .expect("Embedded game-configs.toml")
                .contents_utf8()
                .expect("UTF-8 game-configs.toml"),
        )
        .expect("Embedded game-configs.toml must match the schema");
        assert_eq!(configs.schema, 1, "Unsupported game-configs.toml schema");
        configs
    })
}

impl GameProfile {
    pub fn fex_default(appid: &str) -> Self {
        game_configs()
            .fex_profiles
            .get(appid)
            .cloned()
            .unwrap_or_default()
    }
    /// Fill absent fields without overriding explicit false or auto choices.
    pub fn with_defaults(self, defaults: Self) -> Self {
        Self {
            upscaler: crate::upscaler::Settings::merge(self.upscaler, defaults.upscaler),
            frame_generation: self.frame_generation.or(defaults.frame_generation),
            hud: self.hud.or(defaults.hud),
            engine: self.engine.or(defaults.engine),
            use_steam_shim: self.use_steam_shim.or(defaults.use_steam_shim),
            dxvk_path: self.dxvk_path.or(defaults.dxvk_path),
            vulkan_icd: self.vulkan_icd.or(defaults.vulkan_icd),
            overlay: self.overlay.or(defaults.overlay),
            wine: self.wine.or(defaults.wine),
            emulator: self.emulator.or(defaults.emulator),
            graphics_api: self.graphics_api.or(defaults.graphics_api),
            steam_protocol: self.steam_protocol.or(defaults.steam_protocol),
            steam_bridge: self.steam_bridge.or(defaults.steam_bridge),
        }
    }

    pub fn load(path: &Path) -> Result<Option<Self>> {
        crate::configuration::optional_json(path).with_context(|| format!(
            "Invalid launch configuration {}. Correct its JSON field names/types or remove the file to reset it", path.display()
        ))
    }

    pub fn apply_protocol(&self, runtime: &mut ResolvedRuntime) -> Result<()> {
        if let Some(protocol) = self.steam_protocol {
            ensure!(
                runtime.steam_protocols.contains(&protocol),
                "Selected runtime does not advertise the requested Steam protocol; choose a supported protocol or runtime"
            );
            runtime.steam_protocol = protocol;
        }
        Ok(())
    }

    /// A routing preference must agree with the game's own API setting. Never
    /// select DX12 merely because a multi-API executable happens to import it.
    pub fn graphics_api(&self, exe: &Path, detected: &GraphicsApi) -> Result<GraphicsApi> {
        let configured = configured_graphics_api(exe)?;
        let preferred = self.graphics_api.and_then(GraphicsPreference::api);
        if let (Some(preferred), Some(configured)) = (&preferred, &configured) {
            ensure!(
                preferred == configured,
                "Game is configured for {configured:?}, but graphics_api requests {preferred:?}. Change the game's graphics setting to match or remove graphics_api from its profile"
            );
        }
        Ok(preferred.or(configured).unwrap_or_else(|| detected.clone()))
    }
}

fn configured_graphics_api(exe: &Path) -> Result<Option<GraphicsApi>> {
    let Some(exe_name) = exe.file_name() else {
        return Ok(None);
    };
    let Some(rule) = game_configs()
        .graphics_api_configs
        .iter()
        .find(|r| exe_name.to_string_lossy().eq_ignore_ascii_case(&r.executable))
    else {
        return Ok(None);
    };
    let path = exe.with_file_name(&rule.config_file);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Read game graphics configuration {}", path.display()));
        }
    };
    let target_section = format!("[{}]", rule.section);
    let mut in_section = false;
    let mut api = None;
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line.eq_ignore_ascii_case(&target_section);
        }
        if in_section
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case(&rule.key)
        {
            api = rule
                .values
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(value.trim()))
                .and_then(|(_, pref)| pref.api());
        }
    }
    Ok(api)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_profiles_preserve_defaults_and_explicit_false() {
        let profile: GameProfile =
            serde_json::from_str(r#"{"overlay":true,"use_steam_shim":false}"#).unwrap();
        let profile = profile.with_defaults(GameProfile::fex_default("690790"));
        assert_eq!(profile.overlay, Some(true));
        assert_eq!(profile.use_steam_shim, Some(false));
        assert!(matches!(
            profile.steam_protocol,
            Some(SteamProtocol::Bridge)
        ));
        assert!(matches!(
            profile.graphics_api,
            Some(GraphicsPreference::Dx11)
        ));
    }

    #[test]
    fn api_preference_respects_game_configuration_and_schema_rejects_typos() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("DevilMayCry5.exe");
        fs::write(
            exe.with_file_name("dmc5config.ini"),
            "[Render]\r\nCapability=DirectX12\r\nTargetPlatform=DirectX11\r\n",
        )
        .unwrap();
        let mut profile = GameProfile::default();
        assert_eq!(
            profile.graphics_api(&exe, &GraphicsApi::DirectX12).unwrap(),
            GraphicsApi::DirectX11
        );
        profile.graphics_api = Some(GraphicsPreference::Dx12);
        assert!(profile.graphics_api(&exe, &GraphicsApi::DirectX12).is_err());
        assert!(serde_json::from_str::<GameProfile>(r#"{"graphics_api":"typo"}"#).is_err());
        assert!(serde_json::from_str::<GameProfile>(r#"{"steam_protocol":"typo"}"#).is_err());
        assert!(serde_json::from_str::<GameProfile>(r#"{"graphcs_api":"dx12"}"#).is_err());
    }
}
