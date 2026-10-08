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
    let wine_eng = info.wine_engine();
    println!("  Wine Auto Runner Pipeline: {}", wine_eng.display_name());
    println!(
        "  GPTK Option Pipeline:      {}",
        nucleon_core::detector::TargetEngine::Gptk.display_name()
    );
    Ok(())
}
