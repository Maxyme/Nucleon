use super::ui;
use anyhow::Result;
use nucleon_core::{d7vk, guard, paths, runner, steam, vkd3d, wine};
use std::fs;

pub fn run() -> Result<()> {
    ui::header("Nucleon Status Report");
    ui::status_bool(
        "Steam.app installed:",
        steam::check_steam_installed().is_ok(),
    );
    ui::status_bool("Steam patched for Nucleon:", steam::is_steam_patched());
    ui::kv(
        "Steam process running:",
        if steam::is_steam_running() {
            "● Running"
        } else {
            "○ Stopped"
        },
    );

    let hook_log = paths::support_dir().join("nucleon-hook.log");
    if hook_log.is_file() {
        if let Ok(c) = fs::read_to_string(&hook_log) {
            if c.contains("All compatibility hooks and instrumentation installed successfully") {
                ui::kv(
                    "Hook Injection Status:",
                    "✓ Active (all compat hooks installed)",
                );
            } else if let Some(last) = c.lines().rev().find(|l| !l.trim().is_empty()) {
                ui::kv("Hook Injection Status:", format!("● {}", last.trim()));
            }
        }
    } else {
        ui::kv(
            "Hook Injection Status:",
            "○ No hook log found yet (restart Steam to inject)",
        );
    }

    let runner_cur = paths::current_runner();
    if runner_cur.exists() {
        ui::kv(
            "Active Default Runner:",
            format!("✓ {}", runner_cur.display()),
        );
    } else {
        ui::kv(
            "Active Default Runner:",
            "✗ Not configured (run 'nucleon setup')",
        );
    }

    println!("  Tri-Engine Architecture & Steam Compatibility Tools:");
    if steam::is_auto_tool_registered() {
        ui::sub_kv("Automatic Engine Router:", "✓ Active");
        ui::tree_kv(
            "└─",
            "Registered in Steam UI:",
            "✓ 'Nucleon (Wine + Automatic Graphics Backend)'",
        );
    }
    if let Some(gptk) = runner::find_gptk_runner() {
        ui::sub_kv("Apple GPTK 4 (DX11/12):", format!("✓ {}", gptk.display()));
    } else if let Ok(Some((fw, _))) = runner::find_gptk_components(None) {
        ui::sub_kv("Apple GPTK 4 (Components):", format!("✓ {}", fw.display()));
    } else {
        println!("    ○ Apple GPTK 4 (DX11/12):        ✗ Not found (run 'nucleon setup' or 'nucleon gptk set-path <DIR>')");
    }
    if steam::is_gptk_tool_registered() {
        ui::tree_kv(
            "└─",
            "Registered in Steam UI:",
            "✓ 'Nucleon (GPTK Wine + Apple D3DMetal)'",
        );
    } else {
        ui::tree_kv(
            "└─",
            "Registered in Steam UI:",
            "○ Not registered (run 'nucleon setup')",
        );
    }

    if let Some(info) = runner::get_kosmickrisp_info() {
        let custom_tag = if info.is_custom { " [CUSTOM]" } else { "" };
        ui::sub_kv(
            format!("Mesa KosmicKrisp (Vulkan {}):", info.api_version),
            format!("✓ {}{}", info.icd_path.display(), custom_tag),
        );
    } else {
        println!("    ○ Mesa KosmicKrisp (Vulkan):     ○ Optional (run 'nucleon kosmickrisp set-path <DIR>' or install Vulkan SDK)");
    }
    if steam::is_kosmickrisp_tool_registered() {
        ui::tree_kv(
            "├─",
            "Registered in Steam UI:",
            "✓ 'Nucleon (Wine + Mesa KosmicKrisp Vulkan)'",
        );
    } else {
        ui::tree_kv(
            "├─",
            "Registered in Steam UI:",
            "○ Not registered (run 'nucleon setup --kosmickrisp')",
        );
    }
    if let Some(vkd3d) = vkd3d::find_vkd3d_proton() {
        let ver = vkd3d.version.as_deref().unwrap_or("detected");
        ui::tree_kv(
            "├─",
            "VKD3D-Proton (Direct3D 12):",
            format!("✓ {} [{}]", vkd3d.root.display(), ver),
        );
    } else {
        ui::tree_kv(
            "├─",
            "VKD3D-Proton (Direct3D 12):",
            "○ Optional (run 'nucleon vkd3d set-path <DIR>' or 'nucleon setup --vkd3d-path <DIR>')",
        );
    }
    if let Some(d7vk) = d7vk::find_d7vk() {
        let ver = d7vk.version.as_deref().unwrap_or("detected");
        ui::tree_kv(
            "└─",
            "D7VK (DirectDraw / D3D 1-7):",
            format!("✓ {} [{}]", d7vk.root.display(), ver),
        );
    } else {
        ui::tree_kv(
            "└─",
            "D7VK (DirectDraw / D3D 1-7):",
            "○ Optional (run 'nucleon d7vk fetch' or 'nucleon setup --fetch-d7vk')",
        );
    }

    let active_wine = wine::get_active_wine_runtime();
    let all_wines = wine::discover_wine_runtimes();
    if let Some(ref aw) = active_wine {
        let ver_str = aw.version.as_deref().unwrap_or("detected");
        ui::sub_kv(
            "Wine (DX9/10/Legacy):",
            format!("✓ {} [{}]", aw.name, ver_str),
        );
        if steam::is_wine_tool_registered() {
            ui::tree_kv(
                "├─",
                "Registered in Steam UI:",
                "✓ 'Nucleon (Wine + WineD3D OpenGL)'",
            );
        } else {
            ui::tree_kv(
                "├─",
                "Registered in Steam UI:",
                "○ Not registered (run 'nucleon setup')",
            );
        }
        if all_wines.len() > 1 {
            let other_ids: Vec<String> = all_wines
                .iter()
                .filter(|r| r.root != aw.root)
                .map(|r| r.id.clone())
                .collect();
            ui::tree_kv(
                "└─",
                "Switchable versions:",
                format!("{} (run 'nucleon wine list')", other_ids.join(", ")),
            );
        }
    } else if let Some(staging) = runner::find_wine_staging_runtime() {
        ui::sub_kv(
            "Wine-Staging (DX9/10/Legacy):",
            format!("✓ {}", staging.display()),
        );
        if steam::is_staging_tool_registered() {
            ui::tree_kv(
                "└─",
                "Registered in Steam UI:",
                "✓ 'Nucleon (Wine + WineD3D OpenGL)'",
            );
        }
    } else {
        println!("    ○ Wine (DX9/10/Legacy):          ○ Optional (install Heroic Wine or brew install --cask wine-staging)");
    }

    let bridge = paths::bridge_dir();
    let has_bridge =
        bridge.join("steamclient64.dll").is_file() && bridge.join("tier0_s64.dll").is_file();
    ui::status_bool("Bridge libraries staged:", has_bridge);

    let guard_status = guard::check_guard_status();
    println!("  Background Steam Update Guard:");
    let la_str = if guard_status.launchagent_loaded {
        format!("✓ Active / Loaded ('{}')", guard::GUARD_LABEL)
    } else if guard_status.launchagent_installed {
        "○ Installed but not loaded (run 'nucleon guard install')".to_string()
    } else {
        "○ Not installed (run 'nucleon guard install')".to_string()
    };
    ui::sub_kv("LaunchAgent Service:", la_str);
    ui::sub_kv(
        "Steam Ad-hoc Signature:",
        if guard_status.adhoc_signed {
            "✓ Valid"
        } else {
            "✗ Invalid / Revoked (run 'nucleon guard run')"
        },
    );
    ui::sub_kv(
        "WebUI Chunk Compatibility:",
        if guard_status.webui_patched {
            "✓ Patched"
        } else {
            "✗ Unpatched (run 'nucleon guard run')"
        },
    );
    ui::sub_kv(
        "Hook Dynamic Injection:",
        if guard_status.plist_patched && guard_status.hook_dylib_present {
            "✓ Active"
        } else {
            "✗ Inactive"
        },
    );
    ui::sub_kv(
        "Dynamic Signature Cache:",
        if guard_status.signatures_cached {
            format!(
                "✓ Cached (build {})",
                guard_status.detected_steam_build.unwrap_or(0)
            )
        } else {
            "○ Auto-generates on launch/guard run".to_string()
        },
    );

    Ok(())
}
