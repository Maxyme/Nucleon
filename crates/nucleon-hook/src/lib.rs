pub mod scanner;
pub mod hooks;

use std::ffi::{CStr, c_char};
use log::{info, warn};
use nucleon_core::{paths, signatures};

#[no_mangle]
pub extern "C" fn nucleon_version() -> *const c_char {
    static VERSION: &[u8] = b"Nucleon 0.2.0 (Rust)\0";
    VERSION.as_ptr() as *const c_char
}

#[ctor::ctor]
fn init() {
    unsafe {
        let progname_ptr = libc::getprogname();
        if progname_ptr.is_null() {
            return;
        }
        let progname = CStr::from_ptr(progname_ptr).to_string_lossy();
        if !progname.contains("steam_osx") {
            // Do not initialize inside arbitrary child helper binaries
            return;
        }
    }

    env_logger::init();
    info!("==> Nucleon hook injected into steam_osx!");

    // 1. Install spawn environment sanitization
    if let Err(e) = hooks::spawn::install_spawn_hooks() {
        warn!("Failed to install spawn hooks: {e:#}");
    }

    // 2. Locate steamclient.dylib in memory
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(500));

        let steamclient = match scanner::find_image("steamclient.dylib") {
            Some(img) => img,
            None => {
                warn!("steamclient.dylib not yet loaded");
                return;
            }
        };

        info!("Found steamclient.dylib at base slide: 0x{:x}", steamclient.slide);

        // 3. Load signature database
        let sig_dir = paths::signatures_dir();
        let sig_db = match signatures::find_latest_signature_db(&sig_dir) {
            Ok(Some((path, db))) => {
                info!("Loaded signatures from {}", path.display());
                db
            }
            _ => {
                warn!("No signature database found in {}", sig_dir.display());
                return;
            }
        };

        let mut init_addr = 0usize;
        let mut is_enabled_addr = 0usize;

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
            }
        }

        if init_addr != 0 || is_enabled_addr != 0 {
            if let Err(e) = hooks::compat::install_compat_hooks(init_addr, is_enabled_addr) {
                warn!("Failed to install compat hooks: {e:#}");
            }
        }
    });
}
