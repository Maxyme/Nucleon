//! Read Steam metadata without modifying the client, its config or installed depots.
use crate::{fs_util::atomic_write_file, paths};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installation {
    pub schema: u32,
    pub appid: u32,
    pub directory: PathBuf,
    pub data_path: PathBuf,
    pub branch: String,
    pub language: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LaunchEntry {
    pub id: String,
    pub description: String,
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
}

pub fn game_directory(appid: u32) -> PathBuf {
    paths::support_dir()
        .join("games/steam")
        .join(appid.to_string())
}

fn receipt_path(appid: u32) -> PathBuf {
    game_directory(appid).join("installation.json")
}

impl Installation {
    pub fn load(appid: u32) -> Result<Option<Self>> {
        let path = receipt_path(appid);
        let Some(text) = crate::configuration::optional_file(&path)? else {
            return Ok(None);
        };
        let installed: Self = serde_json::from_str(&text)
            .with_context(|| format!("Invalid installation record {}", path.display()))?;
        ensure!(
            installed.schema == 1 && installed.appid == appid,
            "Invalid Steam installation identity"
        );
        ensure!(
            installed.directory.is_absolute() && installed.data_path.is_absolute(),
            "Installation paths must be absolute"
        );
        Ok(Some(installed))
    }

    /// Publish only after the downloader has completed and validation has succeeded.
    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(game_directory(self.appid))?;
        atomic_write_file(receipt_path(self.appid), serde_json::to_vec_pretty(self)?)
    }
}

pub fn installed(appid: u32) -> Result<Installation> {
    if let Some(installation) = Installation::load(appid)? {
        ensure!(
            installation.directory.is_dir(),
            "Game directory is missing: {}",
            installation.directory.display()
        );
        return Ok(installation);
    }
    bail!("Game {appid} is not installed. Run 'nucleon steam install {appid} --username <account>'")
}

pub fn launch_entries(appid: u32) -> Result<Vec<LaunchEntry>> {
    let path = paths::steam_data_dir().join("appcache/appinfo.vdf");
    let data = fs::read(&path).with_context(|| {
        format!(
            "Read {}. Open Steam to populate game metadata, or supply --exe",
            path.display()
        )
    })?;
    let vdf = super::appinfo::read_app(&data, appid)?
        .context("Game metadata is not cached; open the game in Steam or supply --exe")?;
    let id = appid.to_string();
    let launch = vdf
        .get_path(&[&id, "appinfo", "config", "launch"])
        .and_then(|v| v.as_obj())
        .context("Game has no cached launch entries; supply --exe")?;
    let mut entries = Vec::new();
    for (id, value) in launch.iter() {
        let Some(entry) = value.as_obj() else {
            continue;
        };
        let Some(exe) = entry.get("executable").and_then(|v| v.as_str()) else {
            continue;
        };
        let os = entry
            .get("config")
            .and_then(|v| v.as_obj())
            .and_then(|o| o.get("oslist"))
            .and_then(|v| v.as_str());
        if !exe.to_ascii_lowercase().ends_with(".exe")
            || os.is_some_and(|s| {
                !s.split(',')
                    .any(|p| p.trim().eq_ignore_ascii_case("windows"))
            })
        {
            continue;
        }
        let args = entry
            .get("arguments")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        entries.push(LaunchEntry {
            id: id.to_string(),
            description: entry
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or(exe)
                .into(),
            executable: exe.into(),
            arguments: windows_arguments(args)?,
            working_directory: entry
                .get("workingdir")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        });
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(entries)
}

pub struct LaunchTarget {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub cwd: PathBuf,
}

pub fn target(
    installation: &Installation,
    executable: Option<&Path>,
    launch_id: Option<&str>,
) -> Result<LaunchTarget> {
    if let Some(exe) = executable {
        let exe = if exe.is_absolute() {
            exe.canonicalize()?
        } else {
            within(&installation.directory, &exe.to_string_lossy())?
        };
        ensure!(
            exe.is_file(),
            "Game executable is missing: {}",
            exe.display()
        );
        return Ok(LaunchTarget {
            cwd: exe.parent().context("EXE has no parent")?.into(),
            executable: exe,
            arguments: vec![],
        });
    }
    let entries = launch_entries(installation.appid)?;
    let selected = if let Some(id) = launch_id {
        entries
            .iter()
            .find(|entry| entry.id == id)
            .with_context(|| {
                format!(
                    "No Windows launch entry {id}; use 'nucleon steam show {}'",
                    installation.appid
                )
            })?
    } else {
        ensure!(
            entries.len() == 1,
            "Expected one Windows launch entry, found {}. Use 'nucleon steam show {}', then --launch-id <id> or --exe <path>",
            entries.len(),
            installation.appid
        );
        &entries[0]
    };
    let executable = within(&installation.directory, &selected.executable)?;
    ensure!(
        executable.is_file(),
        "Windows executable is missing: {}. Install the Windows version or supply --exe",
        executable.display()
    );
    let cwd = selected
        .working_directory
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|p| within(&installation.directory, p))
        .transpose()?
        .unwrap_or_else(|| installation.directory.clone());
    ensure!(
        cwd.is_dir(),
        "Working directory is missing: {}",
        cwd.display()
    );
    Ok(LaunchTarget {
        executable,
        cwd,
        arguments: selected.arguments.clone(),
    })
}

/// Metadata cannot escape an installation via absolute paths, traversal or symlinks.
fn within(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = relative.replace('\\', "/");
    let path = Path::new(&relative);
    ensure!(
        !relative.contains(':')
            && !path.is_absolute()
            && path.components().all(|c| matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )),
        "Invalid game-relative path: {relative}"
    );
    let root = root.canonicalize()?;
    let result = root.join(path).canonicalize()?;
    ensure!(
        result.starts_with(&root),
        "Game path escapes its installation"
    );
    Ok(result)
}

/// Windows quoting rules, without invoking a shell or expanding variables.
fn windows_arguments(input: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut chars = input.chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut word = String::new();
        let mut quoted = false;
        while let Some(&ch) = chars.peek() {
            if ch.is_whitespace() && !quoted {
                break;
            }
            let mut slashes = 0;
            while chars.peek() == Some(&'\\') {
                chars.next();
                slashes += 1;
            }
            if chars.peek() == Some(&'"') {
                word.extend(std::iter::repeat_n('\\', slashes / 2));
                chars.next();
                if slashes % 2 == 1 {
                    word.push('"');
                } else if quoted && chars.peek() == Some(&'"') {
                    chars.next();
                    word.push('"');
                } else {
                    quoted = !quoted;
                }
            } else {
                word.extend(std::iter::repeat_n('\\', slashes));
                if !quoted && chars.peek().is_some_and(|c| c.is_whitespace()) {
                    break;
                }
                if let Some(c) = chars.next() {
                    word.push(c);
                }
            }
        }
        ensure!(
            !quoted,
            "Unterminated quote in Steam launch arguments; supply --exe and explicit arguments"
        );
        result.push(word);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_arguments_keep_windows_paths_empty_values_and_shell_literals() {
        assert_eq!(
            windows_arguments(r#"-game "a b" "" C:\Games\File $(test) `ticks`"#).unwrap(),
            ["-game", "a b", "", "C:\\Games\\File", "$(test)", "`ticks`"]
        );
        assert_eq!(windows_arguments(r#""a\"b""#).unwrap(), ["a\"b"]);
        assert!(windows_arguments("\"unfinished").is_err());
    }
    #[test]
    fn paths_stay_inside_the_game_even_through_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("game");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("game.exe"), b"exe").unwrap();
        assert!(within(&root, "game.exe").is_ok());
        for path in ["../elsewhere", "/tmp/other", "C:\\Game.exe"] {
            assert!(within(&root, path).is_err());
        }
        #[cfg(unix)]
        {
            fs::write(dir.path().join("outside.exe"), b"exe").unwrap();
            std::os::unix::fs::symlink(dir.path().join("outside.exe"), root.join("link.exe"))
                .unwrap();
            assert!(within(&root, "link.exe").is_err());
        }
    }
}
