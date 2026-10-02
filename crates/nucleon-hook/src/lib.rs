pub mod logger;
pub mod scanner;
pub mod hooks;

use std::ffi::{CStr, c_char};
use std::sync::OnceLock;
use log::{info, warn};
use nucleon_core::{paths, signatures};
use frida_gum::Gum;

pub static GUM: OnceLock<Gum> = OnceLock::new();

pub fn get_gum() -> &'static Gum {
    GUM.get_or_init(Gum::obtain)
}

#[no_mangle]
pub extern "C" fn nucleon_version() -> *const c_char {
    static VERSION: &[u8] = b"Nucleon 0.2.0 (Rust)\0";
    VERSION.as_ptr() as *const c_char
}

#[ctor::ctor]
fn init() {
    let progname = unsafe {
        let progname_ptr = libc::getprogname();
        if progname_ptr.is_null() {
            return;
        }
        CStr::from_ptr(progname_ptr).to_string_lossy().to_string()
    };

    if !progname.contains("steam_osx") && !progname.contains("Steam Helper") {
        return;
    }

    if progname.contains("Steam Helper") {
        let log_file = paths::support_dir().join("nucleon-helper.log");
        let _ = logger::init(log_file);
        info!("==> Nucleon hook injected into Steam Helper (pid {})", std::process::id());
        if let Err(e) = hooks::webpatch::install_webpatch_hooks() {
            warn!("Failed to install webpatch hooks in Steam Helper: {e:#}");
        }
        return;
    }

    let log_file = paths::support_dir().join("nucleon-hook.log");
    let _ = logger::init(log_file);
    info!("==> Nucleon hook injected into steam_osx (pid {})", std::process::id());

    // Export STEAM_EXTRA_COMPAT_TOOLS_PATHS so Steam searches compatibilitytools.d
    unsafe {
        let tools_path = paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d");
        if let Ok(c_path) = std::ffi::CString::new(tools_path.to_string_lossy().as_bytes()) {
            libc::setenv(c"STEAM_EXTRA_COMPAT_TOOLS_PATHS".as_ptr(), c_path.as_ptr(), 1);
            info!("Set STEAM_EXTRA_COMPAT_TOOLS_PATHS={}", tools_path.display());
        }
    }

    // 1. Install spawn environment sanitization
    if let Err(e) = hooks::spawn::install_spawn_hooks() {
        warn!("Failed to install spawn hooks: {e:#}");
    }

    // 2. Install filesystem permission guards (prevents "missing file privileges" on Steam depots)
    if let Err(e) = hooks::fs::install_fs_hooks() {
        warn!("Failed to install fs hooks: {e:#}");
    }

    // 3. Install WebUI webpatch hooks (enables Install button and Compatibility settings)
    if let Err(e) = hooks::webpatch::install_webpatch_hooks() {
        warn!("Failed to install webpatch hooks: {e:#}");
    }

    // 4. Locate steamclient.dylib in memory with retry loop
    std::thread::spawn(|| {
        info!("Waiting for steamclient.dylib to load...");
        let mut steamclient = None;
        for _ in 1..=120 {
            if let Some(img) = scanner::find_image("steamclient.dylib") {
                steamclient = Some(img);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        let steamclient = match steamclient {
            Some(img) => img,
            None => {
                warn!("steamclient.dylib was not loaded after 60 seconds; giving up");
                return;
            }
        };

        info!("Found steamclient.dylib at base slide: 0x{:x}", steamclient.slide);

        // Load signature database matching installed Steam build
        let sig_dir = paths::signatures_dir();
        let sig_db = match signatures::find_latest_signature_db(&sig_dir) {
            Ok(Some((path, db))) => {
                info!("Loaded signatures from {} (build {})", path.display(), db.steam_build);
                db
            }
            _ => {
                warn!("No signature database found in {}", sig_dir.display());
                return;
            }
        };

        let mut init_addr = 0usize;
        let mut is_enabled_addr = 0usize;
        let mut oslist_gate_addr = 0usize;
        let mut find_tool_addr = 0usize;

        for sig in &sig_db.signatures {
            if sig.name == "CCompatManager::Init" {
                if let Some(ref hex_addr) = sig.func_addr_this_build {
                    if let Ok(offset) = usize::from_str_radix(hex_addr.trim_start_matches("0x"), 16) {
                        init_addr = (steamclient.slide as usize).wrapping_add(offset);
                    }
                }
            } else if sig.name == "CCompatManager::BIsCompatibilityToolEnabled" {
                if let Some(ref hex_addr) = sig.func_addr_this_build {
                    if let Ok(offset) = usize::from_str_radix(hex_addr.trim_start_matches("0x"), 16) {
                        is_enabled_addr = (steamclient.slide as usize).wrapping_add(offset);
                    }
                }
            } else if sig.name == "CCompatManager::GetOSListOverrideForApp.oslist_gate" {
                if let Some(ref hex_addr) = sig.func_addr_this_build {
                    if let Ok(offset) = usize::from_str_radix(hex_addr.trim_start_matches("0x"), 16) {
                        oslist_gate_addr = (steamclient.slide as usize).wrapping_add(offset);
                    }
                }
            } else if sig.name == "CCompatManager::FindToolForTargetApp" {
                if let Some(ref hex_addr) = sig.func_addr_this_build {
                    if let Ok(offset) = usize::from_str_radix(hex_addr.trim_start_matches("0x"), 16) {
                        find_tool_addr = (steamclient.slide as usize).wrapping_add(offset);
                    }
                }
            }
        }

        info!(
            "Resolved hook addresses: Init=0x{:x}, BIsEnabled=0x{:x}, oslist_gate=0x{:x}, find_tool=0x{:x}",
            init_addr, is_enabled_addr, oslist_gate_addr, find_tool_addr
        );

        if init_addr != 0 || is_enabled_addr != 0 || oslist_gate_addr != 0 || find_tool_addr != 0 {
            if let Err(e) = hooks::compat::install_compat_hooks(init_addr, is_enabled_addr, oslist_gate_addr, find_tool_addr) {
                warn!("Failed to install compat hooks: {e:#}");
            } else {
                info!("All compatibility hooks and instrumentation installed successfully!");
            }
        }
    });
}
