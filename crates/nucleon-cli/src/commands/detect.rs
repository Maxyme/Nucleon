use anyhow::Result;
use std::path::Path;

pub fn run(path: &Path) -> Result<()> {
    println!("==> Analyzing binary / game directory: {}", path.display());
    let info = nucleon_core::detector::detect_target_engine(path);
    println!("  Graphics API detected:     {:?}", info.api);
    println!(
        "  Detected library:          {}",
        info.detected_dll.as_deref().unwrap_or("None (heuristic)")
    );
    println!("  Recommended Engine:        {:?}", info.engine);
    println!(
        "  Target Pipeline:           {}",
        info.engine.display_name()
    );
    Ok(())
}
