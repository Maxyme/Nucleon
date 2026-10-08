pub mod detect;
pub mod gptk;
pub mod guard;
pub mod kosmickrisp;
pub mod launch;
pub mod logs;
pub mod setup;
pub mod status;
pub mod steam;
pub mod ui;
pub mod vkd3d;
pub mod wine;

use std::path::PathBuf;

/// Locates the compiled Nucleon hook dylib.
pub fn find_hook_dylib() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("libnucleon.dylib").exists() {
                return Some(dir.join("libnucleon.dylib"));
            }
            if dir.join("nucleon.dylib").exists() {
                return Some(dir.join("nucleon.dylib"));
            }
        }
    }
    let manifest_release = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release");
    if manifest_release.join("libnucleon.dylib").exists() {
        return Some(manifest_release.join("libnucleon.dylib"));
    }
    if manifest_release.join("nucleon.dylib").exists() {
        return Some(manifest_release.join("nucleon.dylib"));
    }
    None
}
