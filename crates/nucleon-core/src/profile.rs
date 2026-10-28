//! Structured per-game choices shared by Steam launches and diagnostic wrappers.
use crate::repo_assets::ASSETS;
use crate::{
    detector::GraphicsApi,
    runtime::{ResolvedRuntime, SteamProtocol},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
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

impl GameProfile {
    pub fn fex_default(appid: &str) -> Self {
        let mut profiles: std::collections::BTreeMap<String, Self> = serde_json::from_slice(
            ASSETS
                .get_file("arm64-fex-profiles.json")
                .expect("Embedded FEX profiles")
                .contents(),
        )
        .expect("Embedded FEX profiles must match the profile schema");
        profiles.remove(appid).unwrap_or_default()
    }
    /// Fill absent fields without overriding explicit false or auto choices.
    pub fn with_defaults(self, defaults: Self) -> Self {
        Self {
            upscaler: self.upscaler.or(defaults.upscaler),
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
                protocol != SteamProtocol::ValveRedirect
                    || runtime.steam_protocol == SteamProtocol::ValveRedirect,
                "Selected runtime does not advertise the Valve Steam redirect; choose bridge or a redirect-capable runtime"
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
    if !exe
        .file_name()
        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("DevilMayCry5.exe"))
    {
        return Ok(None);
    }
    let path = exe.with_file_name("dmc5config.ini");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Read game graphics configuration {}", path.display()));
        }
    };
    let mut render = false;
    let mut api = None;
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.starts_with('[') {
            render = line.eq_ignore_ascii_case("[Render]");
        }
        if render
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("TargetPlatform")
        {
            api = match value.trim() {
                "DirectX11" => Some(GraphicsApi::DirectX11),
                "DirectX12" => Some(GraphicsApi::DirectX12),
                _ => None,
            };
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
