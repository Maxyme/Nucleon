use super::ui;
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use nucleon_core::steam::{self, library::Installation};
use std::{path::PathBuf, process::Command};

#[derive(Args, Debug)]
pub struct PlayArgs {
    pub appid: u32,
    #[arg(long)]
    pub hud: bool,
    #[arg(long)]
    pub wine: Option<String>,
    #[arg(long)]
    pub emulator: Option<nucleon_core::runtime_selection::Emulator>,
    #[arg(short, long)]
    pub engine: Option<String>,
    #[arg(long)]
    pub upscaler: Option<nucleon_core::upscaler::Backend>,
    /// Explicit Windows EXE, absolute or relative to the installation
    #[arg(long, conflicts_with = "launch_id")]
    pub exe: Option<PathBuf>,
    /// Select an entry listed by 'nucleon steam show <appid>'
    #[arg(long)]
    pub launch_id: Option<String>,
    /// Resolve without launching Wine, writing logs or modifying Steam
    #[arg(long)]
    pub dry_run: bool,
    #[arg(last = true)]
    pub arguments: Vec<String>,
}

#[derive(Args, Debug)]
pub struct DownloadArgs {
    pub appid: u32,
    /// Steam account; password or Steam Guard approval is requested interactively
    #[arg(long)]
    pub username: String,
    /// Installed DepotDownloader executable
    #[arg(long)]
    pub downloader: Option<PathBuf>,
    #[arg(long)]
    pub branch: Option<String>,
    #[arg(long)]
    pub language: Option<String>,
    /// Authenticate with the Steam mobile app's QR scanner
    #[arg(long)]
    pub qr: bool,
    /// Preview without signing in or downloading
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Subcommand, Debug)]
pub enum SteamAction {
    /// Download the Windows game into a Nucleon-owned directory
    Install(DownloadArgs),
    /// Update an installation managed by Nucleon
    Update(DownloadArgs),
    /// Launch directly through Nucleon with Steamworks support
    Play(PlayArgs),
    /// Show cached Windows launch entries and the installation directory
    Show { appid: u32 },
}

pub fn run(action: SteamAction) -> Result<()> {
    match action {
        SteamAction::Play(args) => super::launch::run_launch(args),
        SteamAction::Install(args) => download(args, false),
        SteamAction::Update(args) => download(args, true),
        SteamAction::Show { appid } => {
            if let Ok(installed) = steam::library::installed(appid) {
                ui::kv("Game directory:", installed.directory.display());
                ui::kv("Environment data:", installed.data_path.display());
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&steam::library::launch_entries(appid)?)?
            );
            Ok(())
        }
    }
}

fn downloader_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let managed = nucleon_core::paths::support_dir().join("tools/depotdownloader/DepotDownloader");
    if managed.is_file() {
        return Ok(managed);
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for name in ["depotdownloader", "DepotDownloader"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    anyhow::bail!(
        "DepotDownloader is not installed. Install it from https://github.com/SteamRE/DepotDownloader or supply --downloader <executable>"
    )
}

fn download(args: DownloadArgs, updating: bool) -> Result<()> {
    ensure!(
        args.appid != 0 && !args.username.trim().is_empty(),
        "An AppID and Steam account are required"
    );
    let previous = Installation::load(args.appid)?;
    ensure!(
        !updating || previous.is_some(),
        "This game is not managed by Nucleon; use 'nucleon steam install' first"
    );
    let directory = previous
        .as_ref()
        .map(|p| p.directory.clone())
        .unwrap_or_else(|| steam::library::game_directory(args.appid).join("files"));
    let data_path = previous
        .as_ref()
        .map(|p| p.data_path.clone())
        .unwrap_or_else(|| steam::library::game_directory(args.appid).join("data"));
    let branch = args
        .branch
        .or_else(|| previous.as_ref().map(|p| p.branch.clone()))
        .unwrap_or_else(|| "public".into());
    let language = args
        .language
        .or_else(|| previous.as_ref().map(|p| p.language.clone()))
        .unwrap_or_else(|| "english".into());
    let binary = if args.dry_run {
        args.downloader
            .clone()
            .unwrap_or_else(|| "DepotDownloader".into())
    } else {
        downloader_path(args.downloader)?
    };
    let command = download_arguments(
        args.appid,
        &args.username,
        &directory,
        &branch,
        &language,
        args.qr,
    );
    if args.dry_run {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"program":binary,"arguments":command.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>(),"directory":directory,"steam_client_modified":false})
            )?
        );
        return Ok(());
    }
    std::fs::create_dir_all(&directory)?;

    ui::info(
        "Downloading Windows files. Complete Steam authentication in the downloader; Nucleon does not store your password.",
    );
    let status = Command::new(&binary)
        .args(&command)
        .status()
        .with_context(|| format!("Start downloader {}", binary.display()))?;
    ensure!(
        status.success(),
        "Downloader failed ({status}); installation was not marked complete"
    );
    ensure!(
        walk_executables(&directory)?,
        "Downloader returned success but no Windows executable was found; installation was not marked complete"
    );
    Installation {
        schema: 1,
        appid: args.appid,
        directory: directory.clone(),
        data_path,
        branch,
        language,
    }
    .save()?;
    ui::success(format!(
        "Game {} installed at {}",
        args.appid,
        directory.display()
    ));
    Ok(())
}

fn walk_executables(path: &std::path::Path) -> Result<bool> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() && walk_executables(&entry.path())? {
            return Ok(true);
        }
        if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn download_arguments(
    appid: u32,
    username: &str,
    directory: &std::path::Path,
    branch: &str,
    language: &str,
    qr: bool,
) -> Vec<std::ffi::OsString> {
    let mut args = vec![
        "-app".into(),
        appid.to_string().into(),
        "-os".into(),
        "windows".into(),
        "-all-archs".into(),
        "-username".into(),
        username.into(),
        "-dir".into(),
        directory.as_os_str().into(),
        "-branch".into(),
        branch.into(),
        "-language".into(),
        language.into(),
        "-validate".into(),
        "-loginid".into(),
        std::process::id().to_string().into(),
    ];
    if qr {
        args.push("-qr".into());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn download_selects_windows_and_preserves_argument_boundaries() {
        let args = download_arguments(
            1622910,
            "a user",
            std::path::Path::new("/Games/a ' $(literal)"),
            "public",
            "english",
            false,
        );
        assert!(args.windows(2).any(|a| a == ["-os", "windows"]));
        assert!(args.contains(&std::ffi::OsString::from("/Games/a ' $(literal)")));
        assert!(
            !args
                .iter()
                .any(|a| a == "-password" || a == "-remember-password")
        );
    }
}
