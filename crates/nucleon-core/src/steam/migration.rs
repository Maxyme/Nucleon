//! Explicit, backed-up retirement of the old injected integration. Never reinstalls it.
use crate::{fs_util::atomic_write_file, paths, profile::GameProfile};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[cfg(target_os = "macos")]
const LABEL: &str = "com.nucleon.steam-guard";

pub fn present() -> bool {
    paths::steam_app()
        .join("Contents/MacOS/nucleon.dylib")
        .exists()
        || paths::steam_guard_plist().exists()
        || paths::steam_data_dir()
            .join("compatibilitytools.d")
            .read_dir()
            .is_ok_and(|entries| {
                entries
                    .flatten()
                    .any(|e| owned(&e.file_name().to_string_lossy()))
            })
}
fn owned(name: &str) -> bool {
    name == "nucleon" || name.starts_with("nucleon-")
}

fn backup_file(path: &Path, backup: &Path, name: &str) -> Result<()> {
    if path.is_file() {
        let destination = backup.join(name);
        fs::create_dir_all(destination.parent().context("Backup path has no parent")?)?;
        fs::copy(path, destination)?;
    }
    Ok(())
}

pub fn detach() -> Result<Option<PathBuf>> {
    if !present() {
        return Ok(None);
    }
    ensure!(
        !super::running()?,
        "Quit Steam completely before removing its old integration"
    );
    let config = paths::steam_data_dir().join("config/config.vdf");
    if config.is_file() {
        verify_custom_launchers(&fs::read_to_string(&config)?)?;
    }
    let backup = paths::support_dir().join("backups").join(format!(
        "steam-detach-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    fs::create_dir_all(&backup)?;
    let agent = paths::steam_guard_plist();
    if agent.exists() {
        backup_file(&agent, &backup, "steam-guard.plist")?;
        #[cfg(target_os = "macos")]
        {
            let service = format!("gui/{}/{LABEL}", rustix::process::getuid());
            if std::process::Command::new("launchctl")
                .args(["print", &service])
                .output()?
                .status
                .success()
            {
                let result = std::process::Command::new("launchctl")
                    .args(["bootout", &service])
                    .output()?;
                ensure!(
                    result.status.success(),
                    "Could not stop legacy Steam guard: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
            }
        }
        fs::remove_file(&agent)?;
    }
    // Preserve the native runtime selection independently of the tool being retired.
    let old_selection = paths::steam_fex_compat_tools_dir().join("fex-runtime.json");
    if !crate::runtime_selection::fex_default_file().exists() && old_selection.is_file() {
        let runtime = crate::runtime_selection::load_fex_selection(&old_selection)?;
        crate::runtime_selection::set_fex_default(&runtime)?;
    }
    let config = paths::steam_data_dir().join("config/config.vdf");
    backup_file(&config, &backup, "config.vdf")?;
    if config.is_file() {
        let content = fs::read_to_string(&config)?;
        preserve_engine_choices(&content, &backup)?;
        adopt_windows_installations(&content, &backup)?;
        let (cleaned, changed) = super::vdf_mapping::unmap_all_nucleon_compat_mappings(&content);
        if changed {
            atomic_write_file(&config, cleaned)?;
        }
    }
    let tools = paths::steam_data_dir().join("compatibilitytools.d");
    if tools.is_dir() {
        fs::create_dir_all(backup.join("tools"))?;
        for entry in fs::read_dir(&tools)? {
            let entry = entry?;
            if owned(&entry.file_name().to_string_lossy()) {
                fs::rename(entry.path(), backup.join("tools").join(entry.file_name()))?;
            }
        }
    }
    retire_client_cache(&paths::steam_data_dir(), &backup)?;
    let plist = paths::steam_info_plist();
    if plist.is_file() {
        backup_file(&plist, &backup, "Info.plist")?;
        let mut value = plist::Value::from_file(&plist)?;
        if let Some(environment) = value
            .as_dictionary_mut()
            .and_then(|d| d.get_mut("LSEnvironment"))
            .and_then(|e| e.as_dictionary_mut())
        {
            for key in ["DYLD_INSERT_LIBRARIES", "STEAM_EXTRA_COMPAT_TOOLS_PATHS"] {
                if let Some(text) = environment.get(key).and_then(|v| v.as_string()) {
                    let kept = text
                        .split(':')
                        .filter(|p| !p.contains("nucleon") && !p.contains("Nucleon"))
                        .collect::<Vec<_>>()
                        .join(":");
                    if kept.is_empty() {
                        environment.remove(key);
                    } else {
                        environment.insert(key.into(), plist::Value::String(kept));
                    }
                }
            }
        }
        let mut bytes = Vec::new();
        value.to_writer_xml(&mut bytes)?;
        atomic_write_file(&plist, bytes)?;
    }
    let hook = paths::steam_app().join("Contents/MacOS/nucleon.dylib");
    if hook.is_file() {
        fs::rename(&hook, backup.join("nucleon.dylib"))?;
    }
    Ok(Some(backup))
}

/// Steam tool names encoded backend preferences that may not be in a game profile.
fn preserve_engine_choices(content: &str, backup: &Path) -> Result<()> {
    let vdf =
        steam_vdf_parser::parse_text(content).context("Invalid Steam config; nothing removed")?;
    let Some(mappings) = vdf
        .get_path(&["Software", "Valve", "Steam", "CompatToolMapping"])
        .and_then(|v| v.as_obj())
    else {
        return Ok(());
    };
    for (id, entry) in mappings.iter() {
        let Some(tool) = entry
            .as_obj()
            .and_then(|o| o.get("name"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if id == "0" || id.parse::<u32>().is_err() || !owned(tool) {
            continue;
        }
        let engine = match tool {
            "nucleon-gptk" => "gptk",
            "nucleon-dxmt" => "dxmt",
            "nucleon-dxvk" => "dxvk",
            "nucleon-vkd3d" => "vkd3d",
            "nucleon-kosmickrisp" => "kosmickrisp",
            "nucleon-wine" | "nucleon-staging" => "wine",
            _ => continue,
        };
        let path = paths::game_profiles_dir(false).join(format!("{id}.json"));
        let mut profile = GameProfile::load(&path)?.unwrap_or_default();
        if profile.engine.is_none() {
            backup_file(&path, backup, &format!("profiles/{id}.json"))?;
            profile.engine = Some(engine.into());
            fs::create_dir_all(path.parent().context("Profile path has no parent")?)?;
            atomic_write_file(path, serde_json::to_vec_pretty(&profile)?)?;
        }
    }
    Ok(())
}

/// Retire Steam's update ownership of Windows installations while keeping files and
/// prefixes at their existing paths. Native Mac installations are never adopted.
fn adopt_windows_installations(content: &str, backup: &Path) -> Result<()> {
    let vdf = steam_vdf_parser::parse_text(content)?;
    let Some(mappings) = vdf.get_obj(&["Software", "Valve", "Steam", "CompatToolMapping"]) else {
        return Ok(());
    };
    for (id, entry) in mappings.iter() {
        let Some(tool) = entry
            .as_obj()
            .and_then(|o| o.get("name"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let Ok(appid) = id.parse::<u32>() else {
            continue;
        };
        if appid == 0 || !owned(tool) {
            continue;
        }
        let installation = match super::library::installed(appid) {
            Ok(i) => i,
            Err(_) => continue,
        };
        if super::is_directory_native_mac(&installation.directory, 4) {
            continue;
        }
        let has_game = super::library::launch_entries(appid).is_ok_and(|entries| {
            entries.iter().any(|e| {
                installation
                    .directory
                    .join(e.executable.replace('\\', "/"))
                    .is_file()
            })
        });
        if !has_game {
            continue;
        }
        let Some(manifest) = super::find_steamapps_dirs()
            .into_iter()
            .map(|library| library.join(format!("appmanifest_{appid}.acf")))
            .find(|path| path.is_file())
        else {
            continue;
        };
        fs::create_dir_all(backup.join("manifests"))?;
        let saved = backup
            .join("manifests")
            .join(format!("appmanifest_{appid}.acf"));
        fs::rename(&manifest, &saved)?;
        if let Err(error) = installation.save() {
            fs::rename(&saved, &manifest).context("Restore manifest after adoption failure")?;
            return Err(error);
        }
    }
    Ok(())
}

fn verify_custom_launchers(content: &str) -> Result<()> {
    let vdf = steam_vdf_parser::parse_text(content)?;
    let Some(mappings) = vdf.get_obj(&["Software", "Valve", "Steam", "CompatToolMapping"]) else {
        return Ok(());
    };
    for (id, entry) in mappings.iter() {
        let Some(tool) = entry
            .as_obj()
            .and_then(|o| o.get("name"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if !owned(tool) || id == "0" {
            continue;
        }
        ensure!(
            !tool.contains('/') && !tool.contains('\\'),
            "Invalid compatibility tool name"
        );
        let path = paths::steam_data_dir()
            .join("compatibilitytools.d")
            .join(tool)
            .join("run");
        let Some(text) = crate::configuration::optional_file(&path)? else {
            continue;
        };
        let custom = text.lines().any(|line| {
            line.trim_start().starts_with("if ")
                || line.contains("NUCLEON_SUPPORT_DIR=")
                || line.contains("NUCLEON_GAME_PROFILE=")
        });
        if custom {
            let appid = id
                .parse::<u32>()
                .context("Invalid game identity in custom launcher")?;
            let profile =
                GameProfile::load(&paths::game_profiles_dir(false).join(format!("{appid}.json")))?
                    .unwrap_or_default();
            ensure!(
                super::library::Installation::load(appid)?.is_some()
                    && profile.wine.is_some()
                    && profile.engine.is_some(),
                "Custom launcher {} must be converted to a Nucleon installation record and explicit per-game Wine/engine profile before detaching",
                path.display()
            );
        }
    }
    Ok(())
}

/// Only client program files are retired. Game libraries and account data are siblings.
fn retire_client_cache(steam_data: &Path, backup: &Path) -> Result<()> {
    let cached = steam_data.join("Steam.AppBundle");
    if !cached.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(&cached)?;
    ensure!(
        metadata.is_dir() && !metadata.is_symlink(),
        "Steam client cache must be a directory, not a symlink"
    );
    let destination = backup.join("Steam.AppBundle");
    ensure!(!destination.exists(), "Steam client backup already exists");
    fs::rename(&cached, destination)
        .context("Back up cached Steam program files; game libraries are untouched")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retiring_the_program_cache_preserves_windows_game_files_and_prefixes() {
        let temp = tempfile::tempdir().unwrap();
        let steam = temp.path().join("Steam");
        let cache = steam.join("Steam.AppBundle/Steam/Contents");
        let game = steam.join("steamapps/common/Windows Game");
        let prefix = steam.join("steamapps/compatdata/123/pfx");
        let backup = temp.path().join("backup");
        for path in [&cache, &game, &prefix, &backup] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(cache.join("old-ui.js"), b"modified client").unwrap();
        fs::write(game.join("game.exe"), b"original game").unwrap();
        fs::write(game.join("data.bin"), b"original assets").unwrap();
        fs::write(prefix.join("save.dat"), b"saved progress").unwrap();
        retire_client_cache(&steam, &backup).unwrap();
        assert_eq!(fs::read(game.join("game.exe")).unwrap(), b"original game");
        assert_eq!(fs::read(game.join("data.bin")).unwrap(), b"original assets");
        assert_eq!(
            fs::read(prefix.join("save.dat")).unwrap(),
            b"saved progress"
        );
        assert_eq!(
            fs::read(backup.join("Steam.AppBundle/Steam/Contents/old-ui.js")).unwrap(),
            b"modified client"
        );
        assert!(!steam.join("Steam.AppBundle").exists());
        retire_client_cache(&steam, &backup).unwrap();
        assert_eq!(fs::read(game.join("game.exe")).unwrap(), b"original game");
    }
}
