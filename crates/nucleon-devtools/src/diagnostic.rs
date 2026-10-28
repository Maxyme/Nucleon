use crate::common::{
    FileLock, clean_environment, command, execute, json, p, repo, require_native, set, sha,
    write_json,
};
use anyhow::{Context, Result, ensure};
use nucleon_core::{fs_util::atomic_write_file, profile::GameProfile, runtime::ResolvedRuntime};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct ConfigurationRestore {
    path: PathBuf,
    original: Vec<u8>,
    _lock: FileLock,
    restored: bool,
}
impl ConfigurationRestore {
    pub fn edit(path: &Path, backup: &Path, api: &str) -> Result<Self> {
        let path = path.canonicalize()?;
        let lock = FileLock::configuration(&path)?;
        let original = fs::read(&path)?;
        atomic_write_file(backup, &original)?;
        let changed = replace_settings(&original, api)?;
        let guard = Self {
            path,
            original,
            _lock: lock,
            restored: false,
        };
        atomic_write_file(&guard.path, changed)?;
        Ok(guard)
    }
    pub fn restore(&mut self) -> Result<()> {
        atomic_write_file(&self.path, &self.original)?;
        self.restored = true;
        Ok(())
    }
}
impl Drop for ConfigurationRestore {
    fn drop(&mut self) {
        if !self.restored
            && let Err(error) = self.restore()
        {
            log::error!("Settings restoration failed; retain the diagnostic backup: {error:#}");
        }
    }
}
fn replace_settings(original: &[u8], api: &str) -> Result<Vec<u8>> {
    // Preserve all unrelated bytes and line endings; the complete original is restored.
    let text = std::str::from_utf8(original)?;
    let platform = match api {
        "dx12" => "DirectX12",
        "dx11" => "DirectX11",
        _ => anyhow::bail!("Choose dx11 or dx12"),
    };
    let mut changed = text.to_string();
    for (key, value) in [
        ("Capability", platform),
        ("TargetPlatform", platform),
        ("PCTargetAPIFallback", "Disable"),
    ] {
        let matches: Vec<_> = changed
            .match_indices(&format!("{key}="))
            .filter(|(index, _)| *index == 0 || changed.as_bytes()[index - 1] == b'\n')
            .map(|(i, _)| i)
            .collect();
        ensure!(matches.len() == 1, "Expected one configuration key: {key}");
        let start = matches[0] + key.len() + 1;
        let end = changed[start..]
            .find(['\r', '\n'])
            .map(|n| start + n)
            .unwrap_or(changed.len());
        changed.replace_range(start..end, value);
    }
    Ok(changed.into_bytes())
}

#[allow(clippy::too_many_arguments)]
pub fn launch(
    runtime: &Path,
    bridge: &Path,
    appid: u32,
    exe: &Path,
    compat: &Path,
    profile: &GameProfile,
    log: &Path,
    timeout: Option<u64>,
    debug: bool,
    cli: &Path,
) -> Result<Value> {
    let runtime = ResolvedRuntime::inspect(runtime)?;
    launch_resolved(
        runtime, bridge, appid, exe, compat, profile, log, timeout, debug, cli,
    )
}
#[allow(clippy::too_many_arguments)]
fn launch_resolved(
    runtime: ResolvedRuntime,
    bridge: &Path,
    appid: u32,
    exe: &Path,
    compat: &Path,
    profile: &GameProfile,
    log: &Path,
    timeout: Option<u64>,
    debug: bool,
    cli: &Path,
) -> Result<Value> {
    let temporary = tempfile::tempdir()?;
    let support = temporary.path().join("support");
    let profile_file = support
        .join(
            if runtime.mode == nucleon_core::runtime::ExecutionMode::Arm64Fex {
                "game_profiles/arm64-fex"
            } else {
                "game_profiles"
            },
        )
        .join(format!("{appid}.json"));
    let mut profile = profile.clone();
    profile.steam_bridge = Some(std::path::absolute(bridge)?);
    write_json(&profile_file, &profile)?;
    let record = support
        .join("games/steam")
        .join(appid.to_string())
        .join("installation.json");
    write_json(
        &record,
        &nucleon_core::steam::library::Installation {
            schema: 1,
            appid,
            directory: exe.parent().context("EXE has no parent")?.canonicalize()?,
            data_path: std::path::absolute(compat)?,
            branch: "public".into(),
            language: "english".into(),
        },
    )?;
    let receipt = log.with_extension("runner.json");
    let wine_log = log.with_extension("wine.log");
    let mut env = clean_environment();
    set(&mut env, "NUCLEON_SUPPORT_DIR", &support);
    for (name, value) in [
        ("NUCLEON_WINE_LOG", p(&wine_log)),
        ("NUCLEON_RUN_RECEIPT", p(&receipt)),
        ("NUCLEON_DEBUG", if debug { "1" } else { "0" }.into()),
    ] {
        set(&mut env, name, value);
    }
    if let Some(seconds) = timeout {
        ensure!(seconds > 0, "--timeout must be positive");
        set(&mut env, "NUCLEON_RUN_SECONDS", seconds.to_string());
    }
    let mut cmd = command(
        cli,
        &[
            "launch".into(),
            appid.to_string(),
            "--exe".into(),
            p(exe),
            "--wine".into(),
            p(&runtime.root),
        ],
        Some(&env),
        exe.parent(),
    );
    execute(
        &mut cmd,
        Some(log),
        Duration::MAX,
        0,
        Duration::from_secs(15),
    )?;
    let report = json(&receipt)?;
    ensure!(
        matches!(report["status"].as_str(), Some("bounded" | "exited")),
        "Runner did not finish a supervised launch; see {}",
        receipt.display()
    );
    Ok(
        json!({"prefix":nucleon_core::paths::game_prefix(compat,&runtime),"wine_log":wine_log,"profile":profile,"runner_receipt":receipt,"timings":report["timings"],"shader_caches":report["shader_caches"],"status":if report["status"]=="bounded" {"BOUNDED_RUN_FINISHED"}else{"PROCESS_EXITED_ZERO"}}),
    )
}

pub fn game(args: &crate::DiagnosticArgs) -> Result<()> {
    require_native()?;
    let is_dirt = args.game == "dirt";
    ensure!(is_dirt || args.game == "dmc5", "Choose dirt or dmc5");
    let appid = if is_dirt { 690790 } else { 601150 };
    let game = args.game_dir.clone().unwrap_or_else(|| {
        nucleon_core::paths::home_dir()
            .join("Library/Application Support/Steam/steamapps/common")
            .join(if is_dirt {
                "DiRT Rally 2.0"
            } else {
                "Devil May Cry 5"
            })
    });
    let exe = game
        .join(if is_dirt {
            "dirtrally2.exe"
        } else {
            "DevilMayCry5.exe"
        })
        .canonicalize()
        .context("Game is not installed in --game-dir")?;
    let runtime = match &args.runtime {
        Some(path) => path.clone(),
        None => nucleon_core::runtime_selection::fex_default()?.root,
    };
    let bridge = args
        .bridge
        .clone()
        .unwrap_or_else(nucleon_core::paths::bridge_dir);
    let cli = args
        .cli
        .clone()
        .or_else(|| std::env::var_os("NUCLEON_CLI").map(PathBuf::from))
        .unwrap_or_else(|| repo().join("target/debug/nucleon"));
    ensure!(cli.is_file(), "Build nucleon-cli first or provide --cli");
    let logs = args.work.join("dx12/logs");
    fs::create_dir_all(&logs)?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S-%f").to_string();
    let log = logs.join(format!("{}-{appid}-{stamp}.log", args.game));
    let receipt = log.with_extension("json");
    let backup = logs.join(format!("dmc5-settings-{stamp}.ini"));
    let metadata = exe.metadata()?;
    let mut report = json!({"schema":2,"appid":appid,"runtime":runtime,"game_identity":{"size":metadata.len(),"modified_ns":metadata.mtime() as i128*1_000_000_000+metadata.mtime_nsec() as i128,"changed_ns":metadata.ctime() as i128*1_000_000_000+metadata.ctime_nsec() as i128,"device":metadata.dev(),"inode":metadata.ino()},"game_sha256":if args.debug{Some(sha(&exe)?)}else{None},"status":"PREPARING","gameplay_validation":"NOT_CONFIRMED","production_profile_changed":false,"runner_receipt":log.with_extension("runner.json"),"log":log,"settings_backup":if is_dirt {None}else{Some(&backup)}});
    write_json(&receipt, &report)?;
    let start = Instant::now();
    let mut settings = None;
    let result = (|| {
        if !is_dirt {
            settings = Some(ConfigurationRestore::edit(
                &game.join("dmc5config.ini"),
                &backup,
                &args.api,
            )?);
        }
        let profile: GameProfile = serde_json::from_value(if is_dirt {
            json!({"graphics_api":"dx11","steam_protocol":"bridge","use_steam_shim":true,"overlay":false})
        } else {
            json!({"engine":if args.api=="dx12"{"vkd3d"}else{"dxmt"},"graphics_api":args.api,"steam_protocol":"valve-redirect","use_steam_shim":true,"overlay":false})
        })?;
        launch(
            &runtime,
            &bridge,
            appid,
            &exe,
            &args.work.join(format!("integrated-compatdata/{appid}")),
            &profile,
            &log,
            args.timeout.map(|n| n.get()),
            args.debug,
            &cli,
        )
    })();
    let restore = if let Some(settings) = &mut settings {
        settings.restore()
    } else {
        Ok(())
    };
    report["settings_restored"] = restore.is_ok().into();
    report["elapsed_seconds_including_preparation"] = start.elapsed().as_secs_f64().into();
    let result = result
        .map(|value| {
            report
                .as_object_mut()
                .unwrap()
                .extend(value.as_object().unwrap().clone());
        })
        .and(restore);
    if let Err(error) = &result {
        report["status"] = "FAILED".into();
        report["error"] = format!("{error:#}").into();
    }
    write_json(&receipt, &report)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_cli_receives_isolated_installation_profile_and_runtime_bound() {
        use nucleon_core::{
            backend::GraphicsBackend,
            runtime::{ExecutionMode, GraphicsPaths, GuestAbi, PrefixCompatibility, SteamProtocol},
        };
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("runtime");
        fs::create_dir(&root).unwrap();
        let runtime = ResolvedRuntime {
            root: root.clone(),
            mode: ExecutionMode::Arm64Fex,
            prefix: PrefixCompatibility {
                family: "fixture".into(),
                abi: 1,
            },
            guests: vec![GuestAbi::X64],
            loader: root.join("bin/wine"),
            server: root.join("bin/wineserver"),
            unix_dir: root.join("lib/wine/aarch64-unix"),
            windows_dirs: vec![],
            backends: vec![GraphicsBackend::Auto],
            environment: Default::default(),
            steam_protocol: SteamProtocol::Bridge,
            steam_helper: false,
            graphics: GraphicsPaths::default(),
        };
        let cli = dir.path().join("nucleon");
        fs::write(&cli,r##"#!/bin/sh
test -z "${WINEPREFIX+x}" || exit 20
test -z "${NUCLEON_ENGINE+x}" || exit 21
test -z "${VK_DRIVER_FILES+x}" || exit 22
test "$1" = "launch" || exit 23
test "$2" = "601150" || exit 24
/bin/cp "$NUCLEON_SUPPORT_DIR/game_profiles/arm64-fex/601150.json" "$NUCLEON_RUN_RECEIPT.profile"
/bin/cp "$NUCLEON_SUPPORT_DIR/games/steam/601150/installation.json" "$NUCLEON_RUN_RECEIPT.installation"
printf '%s' "$6" > "$NUCLEON_RUN_RECEIPT.runtime"
printf '%s' "$NUCLEON_RUN_SECONDS" > "$NUCLEON_RUN_RECEIPT.timeout"
printf '{"status":"bounded","timings":{"preparation_ms":25,"runtime_ms":7000,"cleanup_ms":50}}' > "$NUCLEON_RUN_RECEIPT"
"##).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
        let exe = dir.path().join("game.exe");
        fs::write(&exe, []).unwrap();
        let log = dir.path().join("launch.log");
        let profile: GameProfile =
            serde_json::from_value(json!({"graphics_api":"dx12","overlay":false})).unwrap();
        let result = launch_resolved(
            runtime,
            &dir.path().join("bridge"),
            601150,
            &exe,
            &dir.path().join("compat"),
            &profile,
            &log,
            Some(7),
            false,
            &cli,
        )
        .unwrap();
        assert_eq!(result["status"], "BOUNDED_RUN_FINISHED");
        assert_eq!(result["timings"]["runtime_ms"], 7000);
        assert!(
            result["prefix"]
                .as_str()
                .unwrap()
                .ends_with("compat/nucleon/arm64-fex/fixture/pfx")
        );
        assert_eq!(
            json(&dir.path().join("launch.runner.json.profile")).unwrap(),
            {
                let mut expected = profile.clone();
                expected.steam_bridge = Some(dir.path().join("bridge"));
                serde_json::to_value(expected).unwrap()
            }
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("launch.runner.json.runtime")).unwrap(),
            p(&root)
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("launch.runner.json.timeout")).unwrap(),
            "7"
        );
    }
    #[test]
    fn exact_settings_are_restored_and_competing_locks_fail() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("game.ini");
        let backup = dir.path().join("backup.ini");
        let original=b"[Render]\r\nCapability=DirectX11\r\nTargetPlatform=DirectX11\r\nPCTargetAPIFallback=Enable\r\n";
        fs::write(&ini, original).unwrap();
        {
            let _guard = ConfigurationRestore::edit(&ini, &backup, "dx12").unwrap();
            assert!(fs::read_to_string(&ini).unwrap().contains("DirectX12"));
            assert!(ConfigurationRestore::edit(&ini, &dir.path().join("other"), "dx11").is_err());
        }
        assert_eq!(fs::read(&ini).unwrap(), original);
        assert_eq!(fs::read(&backup).unwrap(), original);
        assert!(replace_settings(b"Capability=x\nCapability=y\n", "dx12").is_err());
    }
}
