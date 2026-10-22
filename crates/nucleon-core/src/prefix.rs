use crate::paths;
use anyhow::{Context, Result, ensure};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Bootstrap failures never publish a readiness marker or stage game DLL overrides.
pub fn ensure_prefix_for_plan(plan: &crate::launch::LaunchPlan) -> Result<()> {
    let runtime = &plan.runtime;
    let p = &plan.prefix;
    if plan.steam_integration {
        let source = plan
            .bridge
            .join(runtime.unix_name())
            .join("lsteamclient.so");
        ensure!(
            crate::runtime_checks::native_arch(&source, runtime.arch()),
            "Missing or incompatible {:?} Steam Unix bridge: {}",
            runtime.mode,
            source.display()
        );
        let pe = plan
            .bridge
            .join(if plan.is64 {
                "x86_64-windows"
            } else {
                "i386-windows"
            })
            .join("lsteamclient.dll");
        ensure!(
            crate::runtime_checks::pe(Some(&pe), plan.is64),
            "Steam PE bridge has the wrong guest ABI: {}",
            pe.display()
        );
        if plan.use_steam_helper {
            plan.validate_steam_helper()?;
        }
        if runtime.steam_protocol == crate::runtime::SteamProtocol::ValveRedirect {
            ensure!(
                plan.is64
                    && crate::runtime_checks::pe(
                        Some(&plan.bridge.join("steamclient64.dll.valve")),
                        true
                    ),
                "Valve redirect requires a genuine AMD64 steamclient64.dll.valve and an x64 guest"
            );
        }
    }
    ensure_runtime_abi(p, runtime.mode)?;
    let configured = p.join(".nucleon-configured-v3");
    let env = plan.bootstrap_env();
    let bootstrapped = p.join(".nucleon-bootstrapped-v1");
    let runtime_inputs = vec![runtime.loader.clone(), runtime.unix_dir.join("ntdll.so")];
    let bootstrap = || -> Result<()> {
        let result = run_prefix_command(
            runtime.command(&runtime.loader).args(["wineboot", "-i"]),
            &env,
            std::time::Duration::from_secs(120),
        );
        if let Err(error) = result {
            let _ = run_prefix_command(
                runtime.command(&runtime.server).arg("-k"),
                &env,
                std::time::Duration::from_secs(5),
            );
            return Err(error.context("Wine prefix initialization failed"));
        }
        ensure!(
            p.join("drive_c/windows").is_dir(),
            "Wine bootstrap did not create a Windows directory"
        );
        crate::fs_util::atomic_write_file(&bootstrapped, b"1")
    };
    if runtime.mode == crate::runtime::ExecutionMode::Arm64Fex {
        if !p.join("drive_c/windows/system32").is_dir()
            || !p.join("user.reg").is_file()
            || !p.join("system.reg").is_file()
        {
            crate::backend_files::remove(&bootstrapped)?;
            crate::backend_files::remove(&configured)?;
        }
        crate::staging::component(
            &p.join(".nucleon-prepared/bootstrap.json"),
            crate::staging::key(&("bootstrap-v2", &runtime.root, &runtime.environment))?,
            &runtime_inputs,
            std::slice::from_ref(&bootstrapped),
            bootstrap,
        )?;
    } else if !p.join("drive_c/windows").is_dir() {
        bootstrap()?;
    }
    ensure!(
        p.join("drive_c/windows").is_dir(),
        "Wine bootstrap did not create a Windows directory"
    );
    isolate_user_shell_folders(p)?;
    let registry = if plan.steam_integration {
        format!("{PREFIX_REGISTRY}\n{STEAM_REGISTRY}")
    } else {
        PREFIX_REGISTRY.to_owned()
    };
    crate::staging::component(
        &p.join(".nucleon-prepared/registry.json"),
        crate::staging::key(&(&registry, &runtime.root))?,
        &runtime_inputs,
        std::slice::from_ref(&configured),
        || {
            let reg = p.join("nucleon_tweaks.reg");
            crate::fs_util::atomic_write_file(&reg, &registry)?;
            run_prefix_command(
                runtime.command(&runtime.loader).arg("regedit").arg(&reg),
                &env,
                std::time::Duration::from_secs(30),
            )?;
            finish_registry_import(&reg, &configured)
        },
    )?;
    if plan.steam_integration {
        prepare_bridge_libraries_for_runtime(p, &plan.bridge, runtime)?;
    }
    if plan.steam_integration
        && runtime.steam_protocol == crate::runtime::SteamProtocol::ValveRedirect
    {
        let marker = p.join(".nucleon-steam-redirect-v1");
        crate::staging::component(
            &p.join(".nucleon-prepared/steam-registry.json"),
            crate::staging::key(&("steam-redirect-registry-v1", &runtime.root))?,
            &runtime_inputs,
            std::slice::from_ref(&marker),
            || {
                for (key, name, value) in [
                    (
                        "Software\\Valve\\Steam",
                        "SteamPath",
                        "C:\\Program Files (x86)\\Steam",
                    ),
                    (
                        "Software\\Valve\\Steam\\ActiveProcess",
                        "SteamClientDll64",
                        "C:\\Program Files (x86)\\Steam\\steamclient64.dll",
                    ),
                    (
                        "Software\\Valve\\Steam\\ActiveProcess",
                        "Universe",
                        "Public",
                    ),
                ] {
                    run_prefix_command(
                        runtime.command(&runtime.loader).args([
                            "reg",
                            "add",
                            &format!("HKCU\\{key}"),
                            "/v",
                            name,
                            "/t",
                            "REG_SZ",
                            "/d",
                            value,
                            "/f",
                        ]),
                        &env,
                        std::time::Duration::from_secs(30),
                    )?;
                }
                crate::fs_util::atomic_write_file(&marker, b"1")
            },
        )?;
    }
    Ok(())
}

/// Record ABI ownership only when absent; formatting does not change ownership.
fn ensure_runtime_abi(prefix: &Path, mode: crate::runtime::ExecutionMode) -> Result<()> {
    let marker = prefix.join(".nucleon-runtime-abi");
    match fs::read_to_string(&marker) {
        Ok(previous) => {
            let recorded: crate::runtime::ExecutionMode = serde_json::from_str(&previous)
                .with_context(|| format!("Invalid prefix ABI marker {}", marker.display()))?;
            ensure!(
                recorded == mode,
                "Prefix belongs to {}; select a separate prefix for {} (ABI marker {})",
                previous.trim(),
                serde_json::to_string(&mode)?,
                marker.display()
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure!(
                !prefix.join("drive_c").exists() || mode == crate::runtime::ExecutionMode::Intel,
                "Existing prefix {} has no native runtime ABI record; use a new ARM64/FEX prefix",
                prefix.display()
            );
            fs::create_dir_all(prefix)?;
            // This records ABI ownership, not readiness; interrupted bootstrap can retry.
            crate::fs_util::atomic_write_file(&marker, serde_json::to_vec(&mode)?)?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Read prefix ABI marker {}", marker.display()));
        }
    }
    Ok(())
}

pub(crate) fn run_prefix_command(
    command: &mut Command,
    env: &std::collections::HashMap<String, String>,
    timeout: std::time::Duration,
) -> Result<()> {
    use crate::process::{OwnedChild, TERMINATION_GRACE};
    crate::launch::clean_command(command);
    crate::launch::clean_standalone_command(command);
    command.envs(env);
    if let Some(path) = env.get("NUCLEON_SETUP_LOG") {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        command
            .stdout(std::process::Stdio::from(file.try_clone()?))
            .stderr(std::process::Stdio::from(file));
    }
    let mut child = OwnedChild::spawn(command)?;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            child.terminate(std::time::Duration::ZERO)?;
            ensure!(status.success(), "Wine setup command failed: {status}");
            return Ok(());
        }
        if start.elapsed() >= timeout {
            child.terminate(TERMINATION_GRACE)?;
            anyhow::bail!(
                "Wine setup command timed out after {} seconds",
                timeout.as_secs()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

pub fn ensure_prefix(prefix_dir: &Path, runner_dir: &Path) -> Result<()> {
    let runtime = crate::runtime::ResolvedRuntime::inspect(runner_dir)?;
    let plan = crate::launch::LaunchPlan::new(
        runtime,
        prefix_dir.to_path_buf(),
        crate::detector::TargetEngine::WineStaging,
        Default::default(),
        true,
        Some(false),
        &crate::detector::GraphicsApi::Unknown,
    )?;
    ensure_prefix_for_plan(&plan)
}

/// Isolates user shell folders (Desktop, Downloads, Documents, etc.) inside the Wine prefix.
/// By default, Wine symlinks these to the user's macOS host directories (`/Users/<user>/Desktop`,
/// `/Users/<user>/Downloads`), which triggers invasive macOS TCC privacy permission prompts
/// ("nucleon would like to access files in your Desktop/Downloads folder") when games start.
///
/// Replacing these symlinks with local isolated directories inside `drive_c/users/<user>/`
/// prevents any host directory access and eliminates TCC permission dialogs.
pub fn isolate_user_shell_folders(prefix_dir: &Path) -> Result<()> {
    let users_dir = prefix_dir.join("drive_c/users");
    if !users_dir.is_dir() {
        return Ok(());
    }

    let folders = [
        "Desktop",
        "Downloads",
        "Documents",
        "Music",
        "Pictures",
        "Videos",
    ];

    if let Ok(entries) = fs::read_dir(&users_dir) {
        for entry in entries.flatten() {
            let user_path = entry.path();
            if user_path.is_dir() {
                for folder in &folders {
                    let target = user_path.join(folder);
                    // Check if target is a symlink (or broken symlink)
                    if target.is_symlink() {
                        let _ = fs::remove_file(&target);
                        let _ = fs::create_dir_all(&target);
                        log::info!(
                            "Isolated Wine user folder: replaced symlink at {} with local directory",
                            target.display()
                        );
                    } else if !target.exists() {
                        let _ = fs::create_dir_all(&target);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Isolates user shell folders across all existing game prefixes in the Steam library.
pub fn isolate_all_steam_game_prefixes() -> Result<usize> {
    let compatdata = paths::steam_compat_data_dir();
    let mut count = 0;
    if compatdata.is_dir()
        && let Ok(entries) = fs::read_dir(&compatdata)
    {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let pfx = p.join("pfx");
                if pfx.is_dir() && isolate_user_shell_folders(&pfx).is_ok() {
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

pub fn configure_prefix_registry(prefix_dir: &Path, runner_dir: &Path) -> Result<()> {
    let runtime = crate::runtime::ResolvedRuntime::inspect(runner_dir)?;
    let plan = crate::launch::LaunchPlan::new(
        runtime,
        prefix_dir.to_path_buf(),
        crate::detector::TargetEngine::WineStaging,
        Default::default(),
        true,
        Some(false),
        &crate::detector::GraphicsApi::Unknown,
    )?;
    let marker = prefix_dir.join(".nucleon-configured-v3");
    if !marker.is_file() {
        let reg = prefix_dir.join("nucleon_tweaks.reg");
        crate::fs_util::atomic_write_file(&reg, format!("{PREFIX_REGISTRY}\n{STEAM_REGISTRY}"))?;
        run_prefix_command(
            plan.runtime
                .command(&plan.runtime.loader)
                .arg("regedit")
                .arg(&reg),
            &plan.bootstrap_env(),
            std::time::Duration::from_secs(30),
        )?;
        finish_registry_import(&reg, &marker)?;
    }
    Ok(())
}

/// Call only after a successful import: temporary-file cleanup is not readiness.
fn finish_registry_import(reg: &Path, configured: &Path) -> Result<()> {
    if let Err(error) = fs::remove_file(reg)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!(
            "Could not remove temporary registry file {} after successful import: {error}",
            reg.display()
        );
    }
    crate::fs_util::atomic_write_file(configured, b"1").with_context(|| {
        format!(
            "Publish registry configuration marker {}",
            configured.display()
        )
    })
}

const PREFIX_REGISTRY: &str = r#"Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\Software\Wine\Mac Driver]
"OpenGLSurfaceMode"="behind"

; DiRT's custom menu cursor is blank on the ARM64/FEX GDI path.
; Use a native pointer for this one resource without changing other cursors.
[HKEY_CURRENT_USER\Software\Wine\Mac Driver\Cursors]
"dirtrally2.exe,104"="arrowCursor"

[HKEY_CURRENT_USER\Software\Wine\Direct3D]
"csmt"=dword:00000001

[HKEY_CURRENT_USER\Software\Wine\DirectSound]
"DefaultCapture"=""
"DefaultVoiceCapture"=""

[HKEY_LOCAL_MACHINE\Software\Microsoft\Windows NT\CurrentVersion\AeDebug]
"Auto"="0"

[HKEY_LOCAL_MACHINE\Software\Wow6432Node\Microsoft\Windows NT\CurrentVersion\AeDebug]
"Auto"="0"

[HKEY_CURRENT_USER\Software\Wine\WineDbg]
"ShowCrashDialog"=dword:00000000

[HKEY_CURRENT_USER\Software\Wine\DllOverrides]
"winemenubuilder.exe"=""

[HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders]
"Desktop"="%USERPROFILE%\\Desktop"
"Personal"="%USERPROFILE%\\Documents"
"{374DE290-123F-4565-9164-39C4925E467B}"="%USERPROFILE%\\Downloads"
"My Music"="%USERPROFILE%\\Music"
"My Pictures"="%USERPROFILE%\\Pictures"
"My Video"="%USERPROFILE%\\Videos"
"#;

const STEAM_REGISTRY: &str = r#"
[HKEY_LOCAL_MACHINE\Software\Classes\steam]
"URL Protocol"=""

[HKEY_LOCAL_MACHINE\Software\Classes\steam\shell\open\command]
@="\"C:\\Program Files (x86)\\Steam\\steam.exe\" \"%1\""

"#;

pub fn stage_bridge_libraries(prefix_dir: &Path) -> Result<()> {
    stage_bridge_libraries_from(prefix_dir, &paths::bridge_dir())
}

fn stage_bridge_libraries_from(prefix_dir: &Path, bridge: &Path) -> Result<()> {
    stage_bridge_layout(prefix_dir, bridge, "x86_64-unix", false, None)
}

pub fn stage_bridge_libraries_for_runtime(
    prefix_dir: &Path,
    bridge: &Path,
    runtime: &crate::runtime::ResolvedRuntime,
) -> Result<()> {
    stage_bridge_layout(
        prefix_dir,
        bridge,
        runtime.unix_name(),
        runtime.steam_protocol == crate::runtime::SteamProtocol::ValveRedirect,
        None,
    )
}

/// Stage the final bridge image, reusing a privately bound/signed variant.
/// Unlike prefix initialization this performs no Wine or Steam command.
pub fn prepare_bridge_libraries_for_runtime(
    prefix: &Path,
    bridge: &Path,
    runtime: &crate::runtime::ResolvedRuntime,
) -> Result<()> {
    stage_bridge_layout(
        prefix,
        bridge,
        runtime.unix_name(),
        runtime.steam_protocol == crate::runtime::SteamProtocol::ValveRedirect,
        Some(runtime),
    )
}

fn stage_bridge_layout(
    prefix_dir: &Path,
    bridge: &Path,
    unix_name: &str,
    redirect: bool,
    bound: Option<&crate::runtime::ResolvedRuntime>,
) -> Result<()> {
    use crate::staging::Desired;
    let mut desired = std::collections::BTreeMap::new();
    let steam_dir = prefix_dir.join("drive_c/Program Files (x86)/Steam");
    let legacy_dir = steam_dir.join("legacycompat");
    let sys32 = prefix_dir.join("drive_c/windows/system32");
    let syswow64 = prefix_dir.join("drive_c/windows/syswow64");

    for d in &[&steam_dir, &legacy_dir, &sys32, &syswow64] {
        fs::create_dir_all(d)?;
    }

    if !bridge.is_dir() {
        return Ok(());
    }

    // Copy files to steam_dir and system directories
    let files_to_stage = [
        (
            "steamclient64.dll",
            vec![
                steam_dir.join("steamclient64.dll"),
                sys32.join("steamclient64.dll"),
            ],
        ),
        (
            "steamclient.dll",
            vec![
                steam_dir.join("steamclient.dll"),
                syswow64.join("steamclient.dll"),
            ],
        ),
        ("tier0_s64.dll", vec![steam_dir.join("tier0_s64.dll")]),
        ("tier0_s.dll", vec![steam_dir.join("tier0_s.dll")]),
        ("vstdlib_s64.dll", vec![steam_dir.join("vstdlib_s64.dll")]),
        ("vstdlib_s.dll", vec![steam_dir.join("vstdlib_s.dll")]),
        ("steam.exe", vec![steam_dir.join("steam.exe")]),
        (
            "lsteamclient.dll",
            vec![
                steam_dir.join("lsteamclient.dll"),
                sys32.join("lsteamclient.dll"),
                syswow64.join("lsteamclient.dll"),
            ],
        ),
        (
            "lsteamclient.so",
            vec![
                steam_dir.join("lsteamclient.so"),
                steam_dir.join("steamclient.so"),
                steam_dir.join("steamclient64.so"),
            ],
        ),
    ];

    for (name, targets) in files_to_stage {
        let src_file = bridge.join(name);
        if src_file.is_file() && (name != "lsteamclient.so" || unix_name == "x86_64-unix") {
            for tgt in targets {
                desired.insert(tgt, Desired::Copy(src_file.to_path_buf()));
            }
        }
    }

    // Copy architecture-specific triggers if present
    let x64_lsteam = bridge.join("x86_64-windows/lsteamclient.dll");
    if x64_lsteam.is_file() {
        fs::create_dir_all(steam_dir.join("x86_64-windows"))?;
        // The standalone shim and games must both resolve the macOS forwarding bridge. A Valve
        // Windows client in the Steam directory shadows this bridge and cannot see native Steam.
        for target in [
            sys32.join("lsteamclient.dll"),
            sys32.join("steamclient64.dll"),
            steam_dir.join("lsteamclient.dll"),
            steam_dir.join("steamclient64.dll"),
            steam_dir.join("x86_64-windows/lsteamclient.dll"),
        ] {
            desired.insert(target, Desired::Copy(x64_lsteam.to_path_buf()));
        }
    }
    let i386_lsteam = bridge.join("i386-windows/lsteamclient.dll");
    if i386_lsteam.is_file() {
        // Builtin lookup follows the original lsteamclient name, even when a game loads
        // steamclient.dll. A flat x64 trigger would be selected for a 32-bit game.
        fs::create_dir_all(steam_dir.join("i386-windows"))?;
        for target in [
            syswow64.join("lsteamclient.dll"),
            syswow64.join("steamclient.dll"),
            steam_dir.join("steamclient.dll"),
            steam_dir.join("i386-windows/lsteamclient.dll"),
        ] {
            desired.insert(target, Desired::Copy(i386_lsteam.to_path_buf()));
        }
    }

    if redirect {
        let valve = bridge.join("steamclient64.dll.valve");
        for target in [
            steam_dir.join("steamclient64.dll"),
            sys32.join("steamclient64.dll"),
        ] {
            desired.insert(target, Desired::Copy(valve.to_path_buf()));
        }
    }

    // Copy architecture-specific unix bridge libraries if present
    let x64_unix_lsteam = bridge.join(unix_name).join("lsteamclient.so");
    #[cfg(target_os = "macos")]
    let x64_unix_lsteam = if let Some(runtime) = bound {
        crate::steam_ntdll::cached_library(&x64_unix_lsteam, runtime)?
    } else {
        x64_unix_lsteam
    };
    if x64_unix_lsteam.is_file() {
        desired.insert(
            steam_dir.join("lsteamclient.so"),
            Desired::Copy(x64_unix_lsteam.to_path_buf()),
        );
        desired.insert(
            steam_dir.join("steamclient.so"),
            Desired::Copy(x64_unix_lsteam.to_path_buf()),
        );
        desired.insert(
            steam_dir.join("steamclient64.so"),
            Desired::Copy(x64_unix_lsteam.to_path_buf()),
        );
    }

    // These links are part of the prepared output set, so missing/retargeted
    // aliases are repaired without touching identical libraries.
    if x64_unix_lsteam.is_file() {
        for name in [
            "lsteamclient.so",
            "steamclient.so",
            "steamclient64.so",
            "ntdll.so",
        ] {
            desired.insert(
                steam_dir.join(unix_name).join(name),
                Desired::Link(std::path::PathBuf::from(format!("../{name}"))),
            );
        }
    }
    if let Some(runtime) = bound {
        desired.insert(
            steam_dir.join("ntdll.so"),
            Desired::Link(runtime.unix_dir.join("ntdll.so").canonicalize()?),
        );
        desired.insert(steam_dir.join("ntdll_compat.so"), Desired::Absent);
    }

    // Copy legacycompat files and legacy Steam.dll
    let legacy_src = bridge.join("legacycompat");
    if legacy_src.is_dir()
        && let Ok(entries) = fs::read_dir(&legacy_src)
    {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() {
                let fname = e.file_name();
                desired.insert(legacy_dir.join(&fname), Desired::Copy(p.to_path_buf()));
                if fname == "Steam.dll" {
                    desired.insert(syswow64.join("Steam.dll"), Desired::Copy(p.to_path_buf()));
                }
            }
        }
    }

    crate::staging::files(
        &prefix_dir.join(".nucleon-prepared/bridge-files.json"),
        &desired,
        |op| op(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_bootstrap_has_no_steam_requirement_and_failed_setup_can_retry() {
        use crate::{
            detector::{GraphicsApi, TargetEngine},
            launch::{GraphicsComponents, LaunchPlan},
            runtime::{self, ExecutionMode, SteamProtocol},
        };
        use std::os::unix::fs::PermissionsExt;
        let (temp, mut runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        runtime.steam_protocol = SteamProtocol::ValveRedirect;
        runtime.steam_helper = true;
        let script = r#"#!/bin/sh
case "$1" in
wineboot)
  test ! -f "$WINEPREFIX/fail" || exit 9
  mkdir -p "$WINEPREFIX/drive_c/windows/system32"
  touch "$WINEPREFIX/user.reg" "$WINEPREFIX/system.reg"
  ;;
regedit) cp "$2" "$WINEPREFIX/imported.reg" ;;
esac
"#;
        fs::write(&runtime.loader, script).unwrap();
        fs::set_permissions(&runtime.loader, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&runtime.server, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&runtime.server, fs::Permissions::from_mode(0o755)).unwrap();
        let prefix = temp.path().join("standalone-prefix");
        fs::create_dir(&prefix).unwrap();
        fs::write(prefix.join("fail"), b"1").unwrap();
        let plan = LaunchPlan::new(
            runtime,
            prefix.clone(),
            TargetEngine::WineStaging,
            GraphicsComponents::default(),
            true,
            None,
            &GraphicsApi::Unknown,
        )
        .unwrap()
        .without_steam();
        assert!(plan.prepare_prefix().is_err());
        assert!(!prefix.join(".nucleon-configured-v3").exists());
        fs::remove_file(prefix.join("fail")).unwrap();
        plan.prepare_prefix().unwrap();
        assert!(prefix.join(".nucleon-configured-v3").is_file());
        assert!(
            !fs::read_to_string(prefix.join("imported.reg"))
                .unwrap()
                .contains("Classes\\steam")
        );
        assert!(!prefix.join("drive_c/Program Files (x86)/Steam").exists());
        assert!(!prefix.join(".nucleon-steam-redirect-v1").exists());
    }
    use tempfile::tempdir;

    #[test]
    fn runtime_abi_accepts_equivalent_json_without_rewriting_the_marker() {
        use crate::runtime::ExecutionMode;
        use std::os::unix::fs::MetadataExt;

        let prefix = tempdir().unwrap();
        let marker = prefix.path().join(".nucleon-runtime-abi");
        for (mode, contents) in [
            (ExecutionMode::Intel, "\"intel\""),
            (ExecutionMode::Intel, "\"intel\"\n"),
            (ExecutionMode::Arm64Fex, "\"arm64-fex\""),
            (ExecutionMode::Arm64Fex, " \t\r\n\"arm64-fex\"\r\n "),
            (ExecutionMode::Arm64Fex, r#""\u0061rm64-fex""#),
        ] {
            fs::write(&marker, contents).unwrap();
            let inode = fs::metadata(&marker).unwrap().ino();
            ensure_runtime_abi(prefix.path(), mode).unwrap();
            assert_eq!(fs::read_to_string(&marker).unwrap(), contents);
            assert_eq!(fs::metadata(&marker).unwrap().ino(), inode);
        }
    }

    #[test]
    fn runtime_abi_rejects_incompatible_invalid_and_unreadable_markers() {
        use crate::runtime::ExecutionMode;

        let prefix = tempdir().unwrap();
        let marker = prefix.path().join(".nucleon-runtime-abi");
        for (contents, expected) in [
            ("\"intel\"\n", "Prefix belongs to"),
            ("", "Invalid prefix ABI marker"),
            ("arm64-fex", "Invalid prefix ABI marker"),
            ("\"unknown\"", "Invalid prefix ABI marker"),
            ("\"arm64-fex\" false", "Invalid prefix ABI marker"),
        ] {
            fs::write(&marker, contents).unwrap();
            let error = ensure_runtime_abi(prefix.path(), ExecutionMode::Arm64Fex).unwrap_err();
            let report = format!("{error:#}");
            assert!(report.contains(expected), "{report}");
            assert!(report.contains(&marker.display().to_string()), "{report}");
            assert_eq!(fs::read_to_string(&marker).unwrap(), contents);
        }
        fs::write(&marker, b"\xff").unwrap();
        let error = ensure_runtime_abi(prefix.path(), ExecutionMode::Arm64Fex).unwrap_err();
        assert!(error.to_string().contains("Read prefix ABI marker"));
        assert_eq!(fs::read(&marker).unwrap(), b"\xff");

        fs::remove_file(&marker).unwrap();
        // A directory guarantees a read failure without relying on Unix permissions.
        fs::create_dir(&marker).unwrap();
        let error = ensure_runtime_abi(prefix.path(), ExecutionMode::Arm64Fex).unwrap_err();
        assert!(error.to_string().contains(&marker.display().to_string()));
        assert!(marker.is_dir());
    }

    #[test]
    fn runtime_abi_initializes_only_eligible_unmarked_prefixes() {
        use crate::runtime::ExecutionMode;

        let temp = tempdir().unwrap();
        for mode in [ExecutionMode::Intel, ExecutionMode::Arm64Fex] {
            let prefix = temp.path().join(format!("new-{mode:?}"));
            ensure_runtime_abi(&prefix, mode).unwrap();
            let marker = prefix.join(".nucleon-runtime-abi");
            assert_eq!(
                fs::read(&marker).unwrap(),
                serde_json::to_vec(&mode).unwrap()
            );
            ensure_runtime_abi(&prefix, mode).unwrap();
        }
        let existing = temp.path().join("existing");
        fs::create_dir_all(existing.join("drive_c")).unwrap();
        let marker = existing.join(".nucleon-runtime-abi");
        let error = ensure_runtime_abi(&existing, ExecutionMode::Arm64Fex).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("has no native runtime ABI record")
        );
        assert!(!marker.exists());
        ensure_runtime_abi(&existing, ExecutionMode::Intel).unwrap();
        assert_eq!(fs::read(&marker).unwrap(), b"\"intel\"");
        assert!(ensure_runtime_abi(&existing, ExecutionMode::Arm64Fex).is_err());
        assert_eq!(fs::read(&marker).unwrap(), b"\"intel\"");
    }

    #[test]
    fn native_staging_preserves_valve_image_and_uses_arm64_unix_bridge() {
        let (temp, mut runtime) = crate::runtime::fixture(crate::runtime::ExecutionMode::Arm64Fex);
        runtime.steam_protocol = crate::runtime::SteamProtocol::ValveRedirect;
        let bridge = temp.path().join("bridge");
        for dir in ["aarch64-unix", "x86_64-unix", "x86_64-windows"] {
            fs::create_dir_all(bridge.join(dir)).unwrap();
        }
        fs::write(
            bridge.join("aarch64-unix/lsteamclient.so"),
            b"ARM64 companion",
        )
        .unwrap();
        fs::write(
            bridge.join("x86_64-unix/lsteamclient.so"),
            b"Intel companion",
        )
        .unwrap();
        fs::write(bridge.join("x86_64-windows/lsteamclient.dll"), b"PE bridge").unwrap();
        fs::write(
            bridge.join("steamclient64.dll.valve"),
            b"original Valve image",
        )
        .unwrap();
        let prefix = temp.path().join("prefix");
        stage_bridge_libraries_for_runtime(&prefix, &bridge, &runtime).unwrap();
        let steam = prefix.join("drive_c/Program Files (x86)/Steam");
        assert_eq!(
            fs::read(steam.join("lsteamclient.so")).unwrap(),
            b"ARM64 companion"
        );
        assert_eq!(
            fs::read(steam.join("steamclient64.dll")).unwrap(),
            b"original Valve image"
        );
        assert_eq!(
            fs::read(prefix.join("drive_c/windows/system32/steamclient64.dll")).unwrap(),
            b"original Valve image"
        );
        assert_eq!(
            fs::read(steam.join("lsteamclient.dll")).unwrap(),
            b"PE bridge"
        );
        assert!(steam.join("aarch64-unix/lsteamclient.so").is_file());
        assert!(!steam.join("x86_64-unix").exists());
        assert_eq!(
            fs::read(bridge.join("steamclient64.dll.valve")).unwrap(),
            b"original Valve image"
        );
    }

    #[test]
    fn failed_bootstrap_does_not_publish_readiness_and_can_retry() {
        use std::os::unix::fs::PermissionsExt;
        let (temp, mut runtime) = crate::runtime::fixture(crate::runtime::ExecutionMode::Arm64Fex);
        let script = temp.path().join("failing-loader");
        fs::write(
            &script,
            "#!/bin/sh\nmkdir -p \"$WINEPREFIX/drive_c/windows\"\nexit 7\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        runtime.loader = script;
        let mut plan = crate::launch::LaunchPlan::new(
            runtime,
            temp.path().join("prefix"),
            crate::detector::TargetEngine::WineStaging,
            Default::default(),
            true,
            Some(false),
            &crate::detector::GraphicsApi::Unknown,
        )
        .unwrap();
        plan.bridge = temp.path().join("bridge");
        for dir in ["aarch64-unix", "x86_64-windows"] {
            fs::create_dir_all(plan.bridge.join(dir)).unwrap();
        }
        let mut arm = crate::runtime_checks::test_image("bridge", 1);
        arm[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
        fs::write(plan.bridge.join("aarch64-unix/lsteamclient.so"), arm).unwrap();
        fs::write(
            plan.bridge.join("x86_64-windows/lsteamclient.dll"),
            crate::runtime_checks::test_pe_image(0x8664),
        )
        .unwrap();
        for _ in 0..2 {
            let error = format!("{:#}", plan.prepare_prefix().unwrap_err());
            assert!(error.contains("initialization failed"), "{error}");
            assert!(!plan.prefix.join(".nucleon-bootstrapped-v1").exists());
            assert!(!plan.prefix.join(".nucleon-configured-v3").exists());
        }
    }

    #[test]
    fn failed_registry_import_never_marks_prefix_configured() {
        let (temp, _) = crate::runtime::fixture(crate::runtime::ExecutionMode::Intel);
        let prefix = temp.path().join("prefix");
        fs::create_dir(&prefix).unwrap();
        // The fixture's non-executable loader guarantees a setup error without Wine.
        assert!(configure_prefix_registry(&prefix, temp.path()).is_err());
        assert!(!prefix.join(".nucleon-configured-v3").exists());
        assert!(prefix.join("nucleon_tweaks.reg").is_file());
    }

    #[test]
    fn registry_readiness_does_not_depend_on_temporary_file_cleanup() {
        let temp = tempdir().unwrap();
        let reg = temp.path().join("nucleon_tweaks.reg");
        let configured = temp.path().join(".nucleon-configured-v3");

        fs::write(&reg, PREFIX_REGISTRY).unwrap();
        finish_registry_import(&reg, &configured).unwrap();
        assert!(!reg.exists());
        assert_eq!(fs::read(&configured).unwrap(), b"1");

        fs::remove_file(&configured).unwrap();
        finish_registry_import(&reg, &configured).unwrap();
        assert_eq!(fs::read(&configured).unwrap(), b"1");

        fs::remove_file(&configured).unwrap();
        // A directory produces a deterministic cleanup error without permission assumptions.
        fs::create_dir(&reg).unwrap();
        finish_registry_import(&reg, &configured).unwrap();
        assert!(reg.is_dir());
        assert_eq!(fs::read(&configured).unwrap(), b"1");
    }

    #[test]
    fn registry_marker_publication_errors_remain_fatal_after_cleanup() {
        let temp = tempdir().unwrap();
        let reg = temp.path().join("nucleon_tweaks.reg");
        let configured = temp.path().join(".nucleon-configured-v3");
        fs::write(&reg, PREFIX_REGISTRY).unwrap();
        fs::create_dir(&configured).unwrap();

        let error = finish_registry_import(&reg, &configured).unwrap_err();
        let report = format!("{error:#}");
        assert!(
            report.contains("Publish registry configuration marker"),
            "{report}"
        );
        assert!(
            report.contains(&configured.display().to_string()),
            "{report}"
        );
        assert!(!reg.exists());
        assert!(configured.is_dir());
    }

    #[test]
    fn setup_command_preserves_failure_and_times_out_owned_children() {
        let env = std::collections::HashMap::new();
        assert!(
            run_prefix_command(
                Command::new("/bin/sh").args(["-c", "exit 19"]),
                &env,
                std::time::Duration::from_secs(2)
            )
            .is_err()
        );
        let error = run_prefix_command(
            Command::new("/bin/sh").args(["-c", "trap '' TERM; sleep 30 & wait"]),
            &env,
            std::time::Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn steam_bridge_builtin_lookup_preserves_both_architectures_and_bound_unix_library() {
        let dir = tempdir().unwrap();
        let bridge = dir.path().join("bridge");
        let prefix = dir.path().join("prefix");
        for arch in ["i386-windows", "x86_64-windows", "x86_64-unix"] {
            fs::create_dir_all(bridge.join(arch)).unwrap();
        }
        fs::write(
            bridge.join("i386-windows/lsteamclient.dll"),
            b"32-bit bridge",
        )
        .unwrap();
        fs::write(
            bridge.join("x86_64-windows/lsteamclient.dll"),
            b"64-bit bridge",
        )
        .unwrap();
        fs::write(bridge.join("x86_64-unix/lsteamclient.so"), b"Unix bridge").unwrap();
        stage_bridge_libraries_from(&prefix, &bridge).unwrap();
        let steam = prefix.join("drive_c/Program Files (x86)/Steam");
        assert_eq!(
            fs::read(steam.join("i386-windows/lsteamclient.dll")).unwrap(),
            b"32-bit bridge"
        );
        assert_eq!(
            fs::read(steam.join("x86_64-windows/lsteamclient.dll")).unwrap(),
            b"64-bit bridge"
        );
        assert_eq!(
            fs::read(steam.join("steamclient.dll")).unwrap(),
            b"32-bit bridge"
        );
        assert_eq!(
            fs::read(steam.join("steamclient64.dll")).unwrap(),
            b"64-bit bridge"
        );
        #[cfg(unix)]
        {
            crate::fs_util::atomic_write_file(steam.join("lsteamclient.so"), b"Bound Unix bridge")
                .unwrap();
            assert_eq!(
                fs::read(steam.join("x86_64-unix/lsteamclient.so")).unwrap(),
                b"Bound Unix bridge"
            );
            assert_eq!(
                fs::read_link(steam.join("x86_64-unix/ntdll.so")).unwrap(),
                Path::new("../ntdll.so")
            );
        }
        stage_bridge_libraries_from(&prefix, &bridge).unwrap();
    }

    #[test]
    fn test_stage_bridge_libraries_structure() {
        let prefix_dir = tempdir().unwrap();
        let bridge_dir = tempdir().unwrap();
        stage_bridge_libraries_from(prefix_dir.path(), bridge_dir.path()).unwrap();

        let steam_dir = prefix_dir.path().join("drive_c/Program Files (x86)/Steam");
        assert!(steam_dir.is_dir());
        let sys32 = prefix_dir.path().join("drive_c/windows/system32");
        assert!(sys32.is_dir());
        let syswow64 = prefix_dir.path().join("drive_c/windows/syswow64");
        assert!(syswow64.is_dir());
    }

    #[test]
    fn test_isolate_user_shell_folders() {
        let prefix_dir = tempdir().unwrap();
        let user_dir = prefix_dir.path().join("drive_c/users/testuser");
        fs::create_dir_all(&user_dir).unwrap();

        // Create a dummy host directory outside the prefix
        let host_dir = tempdir().unwrap();
        let host_desktop = host_dir.path().join("Desktop");
        let host_downloads = host_dir.path().join("Downloads");
        fs::create_dir_all(&host_desktop).unwrap();
        fs::create_dir_all(&host_downloads).unwrap();

        // Simulate Wine's symlinks
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&host_desktop, user_dir.join("Desktop")).unwrap();
            std::os::unix::fs::symlink(&host_downloads, user_dir.join("Downloads")).unwrap();
            assert!(user_dir.join("Desktop").is_symlink());
            assert!(user_dir.join("Downloads").is_symlink());
        }

        isolate_user_shell_folders(prefix_dir.path()).unwrap();

        let desktop = user_dir.join("Desktop");
        let downloads = user_dir.join("Downloads");
        let documents = user_dir.join("Documents");

        assert!(desktop.is_dir());
        assert!(!desktop.is_symlink(), "Desktop must no longer be a symlink");
        assert!(downloads.is_dir());
        assert!(
            !downloads.is_symlink(),
            "Downloads must no longer be a symlink"
        );
        assert!(documents.is_dir());
    }
}
