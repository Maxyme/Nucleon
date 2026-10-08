use super::ui;
use anyhow::Result;
use nucleon_core::{guard, manifest, paths, runner, signatures, steam, vkd3d, wine};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct SetupArgs {
    pub force: bool,
    pub kosmickrisp: bool,
    pub kosmickrisp_path: Option<PathBuf>,
    pub kosmickrisp_version: Option<String>,
    pub fetch_vkd3d: bool,
    pub vkd3d_path: Option<PathBuf>,
    pub wine: Option<String>,
    pub wine_path: Option<PathBuf>,
    pub gptk_path: Option<PathBuf>,
    pub bridge_path: Option<PathBuf>,
}

pub fn run(args: SetupArgs) -> Result<()> {
    ui::header("Setting up Nucleon with Wine and GPTK 4...");
    paths::ensure_dirs()?;

    if let Some(ref wp) = args.wine_path {
        match wine::set_custom_wine_path(wp) {
            Ok(rt) => {
                ui::success(format!(
                    "Registered custom Wine runtime at {}",
                    rt.root.display()
                ));
                let _ = wine::set_active_wine(&rt.id);
            }
            Err(e) => ui::warn(format!(
                "Failed to set custom Wine path {}: {:#}",
                wp.display(),
                e
            )),
        }
    }

    if let Some(ref gp) = args.gptk_path {
        match runner::set_custom_gptk_path(gp) {
            Ok((fw, shared)) => {
                ui::success(format!(
                    "Registered custom Apple GPTK path at {}",
                    gp.display()
                ));
                ui::tree_kv("└─", "D3DMetal.framework:", fw.display());
                ui::tree_kv("└─", "libd3dshared.dylib:", shared.display());
            }
            Err(e) => ui::warn(format!(
                "Failed to set custom GPTK path {}: {:#}",
                gp.display(),
                e
            )),
        }
    }

    if let Some(ref kp) = args.kosmickrisp_path {
        match runner::set_custom_kosmickrisp_path(kp, args.kosmickrisp_version.as_deref()) {
            Ok(info) => {
                ui::success(format!(
                    "Registered custom KosmicKrisp (Vulkan {}) at {}",
                    info.api_version,
                    info.icd_path.display()
                ));
            }
            Err(e) => ui::warn(format!(
                "Failed to set custom KosmicKrisp path {}: {:#}",
                kp.display(),
                e
            )),
        }
    }

    if let Some(ref desired_wine) = args.wine {
        match wine::set_active_wine(desired_wine) {
            Ok(rt) => ui::success(format!(
                "Set active Wine runtime to: {} [{}]",
                rt.name,
                rt.version.as_deref().unwrap_or("Unknown")
            )),
            Err(e) => ui::warn(format!(
                "Failed to set active Wine '{}': {:#}",
                desired_wine, e
            )),
        }
    }

    if let Some(ref p) = args.vkd3d_path {
        match vkd3d::set_custom_vkd3d_proton_path(p) {
            Ok(bundle) => ui::success(format!(
                "Registered custom VKD3D-Proton path at {}",
                bundle.root.display()
            )),
            Err(e) => ui::warn(format!(
                "Failed to set VKD3D-Proton path {}: {:#}",
                p.display(),
                e
            )),
        }
    } else if args.fetch_vkd3d {
        ui::header("Fetching VKD3D-Proton (Direct3D 12 -> Vulkan)...");
        match vkd3d::fetch_vkd3d_proton(None, None) {
            Ok(bundle) => ui::success(format!("VKD3D-Proton staged at {}", bundle.root.display())),
            Err(e) => ui::warn(format!("Failed to fetch VKD3D-Proton: {:#}", e)),
        }
    }

    if args.kosmickrisp || args.kosmickrisp_path.is_some() {
        std::env::set_var("KOSMICKRISP_FORCE", "1");
        let kk_icd = paths::kosmickrisp_dir().join("libkosmickrisp_icd.json");
        if !kk_icd.exists() && runner::find_kosmickrisp_icd().is_none() {
            let dylib = runner::find_kosmickrisp_driver_dylib();
            let lib_path = dylib
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "libvulkan_kosmickrisp.dylib".to_string());
            let version = args.kosmickrisp_version.as_deref().unwrap_or("1.4.0");

            match runner::write_kosmickrisp_icd(&kk_icd, Path::new(&lib_path), version) {
                Ok(()) => {
                    ui::success(format!(
                        "Staged KosmicKrisp driver manifest (Vulkan {}) at {}",
                        version,
                        kk_icd.display()
                    ));
                }
                Err(e) => {
                    ui::warn(format!("Failed to write KosmicKrisp ICD manifest: {:#}", e));
                }
            }
        }
    }

    // 1. Ensure signature DB for installed Steam client
    ui::header("Ensuring Steam client signature database...");
    match signatures::ensure_signature_db_for_installed_steam() {
        Ok((path, db)) => {
            ui::success(format!(
                "Verified & cached Steam client signatures (build {}) at {}",
                db.steam_build,
                path.display()
            ));
        }
        Err(e) => {
            ui::warn(format!(
                "Note: Could not pre-cache signatures during setup: {e}"
            ));
        }
    }

    // 2. Assemble Wine + GPTK 4 runner
    ui::header("Resolving Wine runtime & GPTK 4 D3DMetal components...");
    let runner_path = runner::assemble_runner(
        args.force,
        args.wine_path.as_deref(),
        args.gptk_path.as_deref(),
    )?;
    ui::success(format!("Runner assembled at: {}", runner_path.display()));

    // 3. Stage Valve bridge packages
    ui::header("Staging Valve bridge packages...");
    manifest::fetch_and_stage_valve_packages(args.bridge_path.as_deref())?;
    ui::success("Valve client bridge libraries staged");

    // 4. Register compatibility tool
    let runner_bin = paths::support_dir().join("nucleon-runner");
    let current_exe = std::env::current_exe()?;
    let runner_src = current_exe.parent().unwrap().join("nucleon-runner");

    if runner_src.exists() {
        fs::copy(&runner_src, &runner_bin)?;
    } else {
        let target_runner =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release/nucleon-runner");
        if target_runner.exists() {
            fs::copy(&target_runner, &runner_bin)?;
        }
    }

    if runner_bin.exists() {
        steam::install_compatibility_tool(&runner_bin)?;
        ui::success("Registered Steam compatibility tool: 'Nucleon (Game Porting Toolkit 4)'");
        if steam::is_kosmickrisp_tool_registered() {
            ui::success("Registered Steam compatibility tool: 'Nucleon (KosmicKrisp)'");
        } else {
            ui::info("Steam compatibility tool 'Nucleon (KosmicKrisp)': not registered (KosmicKrisp not detected; pass --kosmickrisp to enable)");
        }
        if steam::is_wine_tool_registered() {
            let active_name = wine::get_active_wine_runtime()
                .map(|r| {
                    format!(
                        "{} [{}]",
                        r.name,
                        r.version.as_deref().unwrap_or("detected")
                    )
                })
                .unwrap_or_else(|| "default".to_string());
            ui::success(format!(
                "Registered Steam compatibility tool: 'Nucleon (Wine)' -> {}",
                active_name
            ));
        }
    }

    // 5. Restore any installed games that Steam may have unlinked
    let restored = steam::sync_library_folders()?;
    if !restored.is_empty() {
        ui::success(format!(
            "Restored {} game(s) in Steam library: {:?}",
            restored.len(),
            restored
        ));
    }

    // 5b. Sanitize all installed game manifests to Ready to Play (clearing queued updates)
    if let Ok(sanitized) = steam::sanitize_installed_app_manifests() {
        if sanitized > 0 {
            ui::success(format!("Sanitized {} game manifest(s) to 'Ready to Play' (cleared pending update downloads)", sanitized));
        }
    }

    // 6. Patch Steam.app if hook dylib is available
    let hook_dylib = super::find_hook_dylib();
    if let Some(ref dylib) = hook_dylib {
        ui::header(format!("Patching Steam client with {}...", dylib.display()));
        steam::patch_steam(dylib)?;
        ui::success("Steam.app patched and signed with ad-hoc signature");
    } else {
        ui::warn("Run 'just build' or 'cargo build --release' to compile hook dylib before patching Steam");
    }

    // 7. Apply size-preserving SteamUI chunk patches and clear CEF cache
    if let Ok(n) = steam::patch_steamui_chunks() {
        if n > 0 {
            ui::success(format!(
                "Patched {} SteamUI WebUI chunk(s) (size-preserving bypass active)",
                n
            ));
        } else {
            ui::success("SteamUI WebUI compatibility verified (size-preserving bypass active)");
        }
    }

    // 8. Register background Steam Update Guard LaunchAgent
    if let Some(ref dylib) = hook_dylib {
        let _ = fs::copy(dylib, paths::bridge_dir().join("nucleon.dylib"));
    }
    match guard::install_launchagent(None) {
        Ok(p) => {
            ui::success(format!(
                "Background Steam Update Guard LaunchAgent installed & active ({})",
                p.display()
            ));
        }
        Err(e) => {
            ui::warn(format!("Background Steam Update Guard notice: {:#}", e));
        }
    }

    println!(
        r#"
==============================================================================
  Nucleon setup complete!
==============================================================================
To use Nucleon in Steam:
  1. Restart Steam: pkill steam_osx && open -a /Applications/Steam.app
  2. The 'Install' button is now enabled for all Windows games in your library.
  3. Clicking 'Install' begins downloading and routes the game via Nucleon.
  4. To configure a specific runner/engine, right-click the game -> Properties -> Compatibility,
     or use Steam Settings -> Compatibility.
  5. Or launch directly from terminal: nucleon launch <AppID>"#
    );

    Ok(())
}
