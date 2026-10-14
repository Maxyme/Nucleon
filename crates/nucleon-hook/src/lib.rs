pub mod hooks;
pub mod logger;
pub mod scanner;

use frida_gum::Gum;
use log::{info, warn};
use nucleon_core::{paths, steam_contract};
use std::ffi::{CStr, c_char};
use std::sync::OnceLock;

pub static GUM: OnceLock<Gum> = OnceLock::new();

pub fn get_gum() -> &'static Gum {
    GUM.get_or_init(Gum::obtain)
}

#[unsafe(no_mangle)]
pub extern "C" fn nucleon_version() -> *const c_char {
    static VERSION: &[u8] = concat!("Nucleon ", env!("CARGO_PKG_VERSION"), " (Rust)\0").as_bytes();
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
        info!(
            "==> Nucleon hook injected into Steam Helper (pid {})",
            std::process::id()
        );
        if let Err(e) = hooks::fs::install_fs_hooks() {
            warn!("Failed to install fs hooks in Steam Helper: {e:#}");
        }
        return;
    }

    let log_file = paths::support_dir().join("nucleon-hook.log");
    let _ = logger::init(log_file);
    info!(
        "==> Nucleon hook injected into steam_osx (pid {})",
        std::process::id()
    );

    // Steam's launch configuration supplies compatibility-tool paths before native threads start.
    // 1. Install spawn environment sanitization
    if let Err(e) = hooks::spawn::install_spawn_hooks() {
        warn!("Failed to install spawn hooks: {e:#}");
    }

    // 2. Install filesystem permission guards (prevents "missing file privileges" on Steam depots)
    if let Err(e) = hooks::fs::install_fs_hooks() {
        warn!("Failed to install fs hooks: {e:#}");
    }

    // 3. Verify or heal WebUI chunk patches asynchronously before CEF renders
    std::thread::spawn(|| {
        if !nucleon_core::steam::are_steamui_chunks_patched() {
            info!("Unpatched WebUI chunks detected on launch; applying self-healing patches...");
            if let Ok(count) = nucleon_core::steam::patch_steamui_chunks() {
                info!(
                    "Successfully patched {} WebUI chunk(s) during Steam launch",
                    count
                );
            }
        }
    });

    // 4. Locate steamclient.dylib in memory with retry loop
    std::thread::spawn(|| {
        info!("Waiting for steamclient.dylib to load...");
        let mut steamclient = None;
        for _ in 1..=120 {
            match scanner::pin_steam_client() {
                Ok(Some(image)) => {
                    steamclient = Some(image);
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    warn!("Image pinning failed: {error:#}");
                    return;
                }
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

        info!("Pinned Steam client: {}", steamclient.path().display());

        let reviewed = match steam_contract::ensure_contract_for_image(steamclient.path()) {
            Ok((path, reviewed)) => {
                info!(
                    "Validated Steam contract {} for UUID {}",
                    path.display(),
                    reviewed.contract().client.uuid
                );
                reviewed
            }
            Err(error) => {
                warn!("Steam compatibility disabled: {error:#}");
                return;
            }
        };
        match hooks::compat::install_contract(steamclient, &reviewed) {
            Ok(()) => {
                info!("Reviewed compatibility contract installed");
            }
            Err(error) => warn!("Steam contract installation refused: {error:#}"),
        }
    });
}
