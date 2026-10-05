use anyhow::Result;
use log::info;

/// Dynamic file interposition is deprecated in favor of size-preserving on-disk
/// patching, which prevents bootstrapper size check loops while eliminating
/// dyld interposition recursion crashes and memory overhead.
pub fn install_webpatch_hooks() -> Result<()> {
    info!("WebUI patching managed via size-preserving on-disk chunk patches");
    Ok(())
}
